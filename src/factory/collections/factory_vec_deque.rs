use gtk::glib::Sender;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::factory::{Factory, FactoryListView, FactoryPrototype, FactoryView};

#[derive(Debug, PartialEq, Eq)]
/// A dynamic index that updates automatically when items are shifted inside a [`Factory`].
///
/// For example a [`FactoryVecDeque`] has an [`insert`] method that allows users
/// to insert data at arbitrary positions.
/// If we insert at the front all following widgets will be moved by one which would
/// invalidate their indices.
/// To allow widgets in a [`Factory`] to still send messages with valid indices
/// this type ensures that the indices is always up to date.
/// Never send this index as [`uszize`] but always inside of a [`Rc`] to the update function
/// because messages can be queued up and stale by the time they are handled.
///
/// In short: only call [`current_index`] from the update function where you actually need the index as [`usize`].
pub struct DynamicIndex {
    inner: RefCell<usize>,
}

impl DynamicIndex {
    /// Get the current index number.
    ///
    /// This value is updated by the [`Factory`] and might change after each update function.
    pub fn current_index(&self) -> usize {
        *self.inner.borrow()
    }

    #[doc(hidden)]
    fn increment(&self) {
        *self.inner.borrow_mut() += 1;
    }

    #[doc(hidden)]
    fn decrement(&self) {
        *self.inner.borrow_mut() -= 1;
    }

    #[doc(hidden)]
    fn new(index: usize) -> Self {
        DynamicIndex {
            inner: RefCell::new(index),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ChangeType {
    Unchanged,
    Add,
    Remove,
    Recreate,
    Update,
}

impl ChangeType {
    fn apply(&mut self, other: ChangeType) {
        match self {
            ChangeType::Unchanged => {
                *self = other;
            }
            ChangeType::Update => {
                if other != ChangeType::Unchanged {
                    *self = other;
                }
            }
            ChangeType::Add | ChangeType::Recreate => {
                if other == ChangeType::Remove {
                    *self = ChangeType::Remove;
                } else if other != ChangeType::Update {
                    panic!(
                        "Logical error in change tracking. Unexpected change: {:?} <- {:?}",
                        self, other
                    );
                }
            }
            ChangeType::Remove => {
                if other == ChangeType::Add {
                    *self = ChangeType::Recreate;
                } else {
                    panic!(
                        "Logical error in change tracking. Unexpected change: {:?} <- {:?}",
                        self, other
                    );
                }
            }
        }
    }
}

#[derive(Debug)]
struct Change {
    ty: ChangeType,
    index: usize,
}

impl Change {
    fn new(index: usize, ty: ChangeType) -> Self {
        Change { index, ty }
    }
}

#[derive(Debug)]
struct IndexedData<T> {
    inner: T,
    index: Rc<DynamicIndex>,
}

impl<T> IndexedData<T> {
    fn new(data: T, index: usize) -> Self {
        let index = Rc::new(DynamicIndex::new(index));
        IndexedData { inner: data, index }
    }
}

/// A container similar to [`VecDeque`] that implements [`Factory`].
#[derive(Default, Debug)]
pub struct FactoryVecDeque<Data>
where
    Data: FactoryPrototype,
{
    data: VecDeque<IndexedData<Data>>,
    widgets: RefCell<VecDeque<Data::Widgets>>,
    changes: RefCell<Vec<Change>>,
}

impl<Data> FactoryVecDeque<Data>
where
    Data: FactoryPrototype,
{
    /// Create a new [`FactoryVecDeque`].
    pub fn new() -> Self {
        FactoryVecDeque {
            data: VecDeque::new(),
            widgets: RefCell::new(VecDeque::new()),
            changes: RefCell::new(Vec::new()),
        }
    }

    /// Insert an element at the end of a [`FactoryVecDeque`].
    pub fn push_back(&mut self, data: Data) {
        let index = self.data.len();
        let data = IndexedData::new(data, index);
        self.add_change(Change::new(index, ChangeType::Add));
        self.data.push_back(data);
    }

    /// Remove an element at the end of a [`FactoryVecDeque`].
    pub fn pop_back(&mut self) -> Option<Data> {
        let data = self.data.pop_back();
        let index = self.data.len();
        self.add_change(Change::new(index, ChangeType::Remove));
        data.map(|data| data.inner)
    }

    /// Adds an element at the front.
    pub fn push_front(&mut self, data: Data) {
        for elem in &self.data {
            elem.index.increment();
        }
        let index = 0;
        self.add_change(Change::new(index, ChangeType::Add));
        let data = IndexedData::new(data, index);
        self.data.push_front(data);
    }

    /// Removes an element from the front.
    pub fn pop_front(&mut self) -> Option<Data> {
        self.add_change(Change::new(0, ChangeType::Remove));
        let data = self.data.pop_front();
        for elem in &self.data {
            elem.index.decrement();
        }
        data.map(|data| data.inner)
    }

    /// Adds an element at a given index.
    /// All elements with indices greater than or equal to index will be shifted towards the back.
    pub fn insert(&mut self, index: usize, data: Data) {
        for elem in &self.data {
            if elem.index.current_index() >= index {
                elem.index.increment();
            }
        }
        self.add_change(Change::new(index, ChangeType::Add));
        let data = IndexedData::new(data, index);
        self.data.insert(index, data);
    }

    /// Removes an element at a given index.
    pub fn remove(&mut self, index: usize) -> Option<Data> {
        self.add_change(Change::new(index, ChangeType::Remove));
        let data = self.data.remove(index);
        for elem in &self.data {
            if elem.index.current_index() > index {
                elem.index.decrement();
            }
        }
        data.map(|data| data.inner)
    }

    /// Get a reference to data stored at `index`.
    pub fn get(&self, index: usize) -> &Data {
        &self.data[index].inner
    }

    /// Get a mutable reference to data stored at `index`.
    ///
    /// Assumes that the data will be modified and the corresponding widget
    /// needs to be updated.
    pub fn get_mut(&mut self, index: usize) -> &mut Data {
        self.add_change(Change::new(index, ChangeType::Update));

        &mut self.data[index].inner
    }

    fn add_change(&mut self, change: Change) {
        match change.ty {
            ChangeType::Add => {
                for elem in self.changes.borrow_mut().iter_mut() {
                    if elem.index >= change.index {
                        elem.index += 1;
                    }
                }
            }
            ChangeType::Remove => {
                for elem in self.changes.borrow_mut().iter_mut() {
                    if elem.index > change.index {
                        elem.index -= 1;
                    }
                }
            }
            _ => (),
        }
        self.changes.borrow_mut().push(change);
    }

    fn compile_changes(&self) -> Vec<ChangeType> {
        let mut change_map = vec![ChangeType::Unchanged; self.data.len() + 1];

        for change in self.changes.borrow().iter() {
            while change_map.len() < change.index {
                change_map.push(ChangeType::Unchanged);
            }
            change_map[change.index].apply(change.ty);
        }

        change_map
    }
}

impl<Data, View> Factory<Data, View> for FactoryVecDeque<Data>
where
    Data: FactoryPrototype<Factory = Self, View = View>,
    View: FactoryView<Data::Root> + FactoryListView<Data::Root>,
{
    type Key = Rc<DynamicIndex>;

    fn generate(&self, view: &View, sender: Sender<Data::Msg>) {
        let change_map = self.compile_changes();
        for (index, change) in change_map.iter().enumerate() {
            let mut widgets = self.widgets.borrow_mut();

            dbg!(&change);
            match change {
                ChangeType::Unchanged => (),
                ChangeType::Add => {
                    let data = &self.data[index];
                    let widget = data.inner.generate(&data.index, sender.clone());
                    if widgets.is_empty() || index == 0 {
                        view.push_front(Data::get_root(&widget));
                    } else {
                        view.insert_after(
                            Data::get_root(&widget),
                            Data::get_root(&widgets[index - 1]),
                        );
                    }
                    widgets.insert(index, widget);
                }
                ChangeType::Update => {
                    let data = &self.data[index];
                    data.inner.update(&data.index, &widgets[index]);
                }
                ChangeType::Remove => {
                    let widget = widgets.remove(index).unwrap();
                    let remove_widget = Data::get_root(&widget);
                    view.remove(remove_widget);
                }
                ChangeType::Recreate => {
                    let widget = widgets.pop_back().unwrap();
                    let remove_widget = Data::get_root(&widget);
                    view.remove(remove_widget);
                    let data = &self.data[index];
                    let widget = data.inner.generate(&data.index, sender.clone());
                    if widgets.is_empty() || index == 0 {
                        view.push_front(Data::get_root(&widget));
                    } else {
                        view.insert_after(
                            Data::get_root(&widget),
                            Data::get_root(&widgets[index - 1]),
                        );
                    }
                    widgets.insert(index, widget);
                }
            }
        }
        self.changes.borrow_mut().clear();
    }
}