use core::marker::PhantomData;
use core::ptr;
use core::iter::{Iterator, IntoIterator, DoubleEndedIterator};
use core::convert::From;
use core::fmt::{self, Debug, Formatter};

use concurrency_toolkit::atomic::{AtomicPtr, Ordering};

use crate::utility::*;
use crate::intrusive_forward_list::IntrusiveForwardListNode;

/// Doubly linked intrusive list node.
///
/// **`self.get_next_ptr()` and `self.get_prev_ptr()` must return different pointers.**
///
/// `T` can either be an immutable reference or a `Sized` object, it is not recommended
/// to return a mutable reference.
///
/// # Safety
///
/// `node` -  __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
/// ADD IT TO THE SAME LIST SIMULTANEOUSLY
/// but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
pub unsafe trait IntrusiveListNode<'a>: IntrusiveForwardListNode<'a> {
    fn get_prev_ptr(&self) -> &AtomicPtr<()>;
}

/// Sample implementation of IntrusiveListNode
#[derive(Debug)]
pub struct IntrusiveListNodeImpl<T> {
    next_ptr: AtomicPtr<()>,
    prev_ptr: AtomicPtr<()>,
    elem: T,
}
impl<T> IntrusiveListNodeImpl<T> {
    pub fn new(elem: T) -> Self {
        let null = ptr::null_mut();

        Self {
            next_ptr: AtomicPtr::new(null),
            prev_ptr: AtomicPtr::new(null),
            elem,
        }
    }
}
impl<T: Default> Default for IntrusiveListNodeImpl<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}
unsafe impl<'a, T: 'a> IntrusiveForwardListNode<'a> for IntrusiveListNodeImpl<T> {
    type Target = &'a T;

    fn get_next_ptr(&self) -> &AtomicPtr<()> {
        &self.next_ptr
    }
    fn get_elem(&'a self) -> Self::Target {
        &self.elem
    }
}
unsafe impl<'a, T: 'a> IntrusiveListNode<'a> for IntrusiveListNodeImpl<T> {
    fn get_prev_ptr(&self) -> &AtomicPtr<()> {
        &self.prev_ptr
    }
}

/// IntrusiveList guarantees that
///  - push and read can be done concurrently while allowing stale read;
///  - deletion can only be done sequentially when there is no
///    writer (excluding the thread doing deletion) or reader.
/// 
/// It is suggested to use this with `RwLock`
pub struct IntrusiveList<'a, Node: IntrusiveListNode<'a>> {
    first_ptr: AtomicPtr<()>,
    last_ptr: AtomicPtr<()>,
    phantom: PhantomData<&'a Node>,
}
impl<'a, Node: IntrusiveListNode<'a> + Debug> Debug for IntrusiveList<'a, Node> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.debug_list().entries(self).finish()
    }
}
impl<'a, Node: IntrusiveListNode<'a>> Default for IntrusiveList<'a, Node> {
    fn default() -> Self {
        Self::new()
    }
}
impl<'a, Node: IntrusiveListNode<'a>> IntrusiveList<'a, Node> {
    pub fn new() -> Self {
        Self {
            first_ptr: AtomicPtr::new(ptr::null_mut()),
            last_ptr: AtomicPtr::new(ptr::null_mut()),
            phantom: PhantomData,
        }
    }

    /// # Safety
    ///
    ///  * `node` -  __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    pub unsafe fn push_back(&self, node: &'a Node) {
        self.push_back_splice(Splice::new_unchecked(node, node));
    }

    /// # Safety
    ///
    ///  * `node` -  __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    pub unsafe fn push_front(&self, node: &'a Node) {
        self.push_front_splice(Splice::new_unchecked(node, node));
    }

    pub fn push_back_splice(&self, splice: Splice<'a, Node>) {
        let null = ptr::null_mut();

        let last_node  = unsafe { &*(splice.last_ptr  as *mut Node as *const Node) };
        let first_node = unsafe { &*(splice.first_ptr as *mut Node as *const Node) };

        last_node.get_next_ptr().store(null, W_ORD);

        loop {
            let last = self.last_ptr.load(R_ORD);

            first_node.get_prev_ptr().store(last, W_ORD);

            let first_node = splice.first_ptr;
            if last.is_null() {
                match self.first_ptr
                    .compare_exchange_weak(null, first_node, RW_ORD, R_ORD)
                {
                    Ok(_) => (),
                    Err(_) => continue,
                }
            } else {
                match unsafe { &*(last as *mut Node as *const Node) }
                    .get_next_ptr()
                    .compare_exchange_weak(null, first_node, RW_ORD, R_ORD)
                {
                    Ok(_) => (),
                    Err(_) => continue,
                }
            }
            let last_node = splice.last_ptr;
            break assert_store_ptr(&self.last_ptr, last, last_node);
        }
    }

    pub fn push_front_splice(&self, splice: Splice<'a, Node>) {
        let null = ptr::null_mut();

        let last_node  = unsafe { &*(splice.last_ptr  as *mut Node as *const Node) };
        let first_node = unsafe { &*(splice.first_ptr as *mut Node as *const Node) };

        first_node.get_prev_ptr().store(null, W_ORD);

        loop {
            let first = self.first_ptr.load(R_ORD);

            last_node.get_next_ptr().store(first, W_ORD);

            let last_node  = splice.last_ptr;
            let first_node = splice.first_ptr;

            if first.is_null() {
                match self.first_ptr
                    .compare_exchange_weak(null, first_node, RW_ORD, R_ORD)
                {
                    Ok(_) => break assert_store_ptr(&self.last_ptr, null, last_node),
                    Err(_) => continue,
                }
            } else {
                match unsafe { &*(first as *mut Node as *const Node) }
                    .get_prev_ptr()
                    .compare_exchange_weak(null, last_node, RW_ORD, R_ORD)
                {
                    Ok(_) => break assert_store_ptr(&self.first_ptr, first, first_node),
                    Err(_) => continue,
                }
            }
        }
    }

    // All methods read the list:

    pub fn iter(&self) -> IntrusiveListIterator<'a, '_, Node> {
        IntrusiveListIterator::from_list(self)
    }

    pub fn is_empty(&self) -> bool {
        self.first_ptr.load(R_ORD).is_null() && self.last_ptr.load(R_ORD).is_null()
    }

    // All methods below are removal methods, which takes the write lock:

    /// Returns `true` if `node` is indeed inside `self`, otherwise `false`.
    ///
    /// # Safety
    ///
    ///  * `node` - it must be in one of the following state:
    ///     - `node.get_next_ptr().is_null() && node.get_prev_ptr().is_null()`
    ///     - `node` is added to `self`
    ///    and, __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    pub unsafe fn remove_node(&mut self, node: &'a Node) -> bool {
        self.splice_impl(node, node).is_some()
    }

    ///  * `f` - return true to remove the node or false to keep it
    /// 
    /// Return (# num of elements left, # num of elements removed)
    pub fn remove_if(&mut self, mut f: impl FnMut(&'a Node) -> bool) -> (usize, usize) {
        use Ordering::Relaxed;

        let mut it = self.first_ptr.load(Relaxed);

        let mut prev: *const Node = ptr::null();
        let mut beg: *const Node = ptr::null();

        let mut cnt = (0, 0);

        while !it.is_null() {
            let node = unsafe { &* (it as *mut Node as *const Node) };
            cnt.0 += 1;
            if f(node) {
                cnt.1 += 1;
                if beg.is_null() {
                    beg = node;
                }
            } else if !beg.is_null() {
                unsafe { self.splice_impl(&* beg, &* prev).unwrap() };
                beg = ptr::null();
            }
            prev = node;
            it = node.get_next_ptr().load(Relaxed);
        }

        cnt.0 -= cnt.1;

        cnt
    }

    pub fn clear(&mut self) {
        use Ordering::Relaxed;

        let null = ptr::null_mut();

        self.first_ptr.store(null, Relaxed);
        self.last_ptr .store(null, Relaxed);
    }

    /// Move all list nodes between `first` and `last` (inclusive) from `self`
    /// and return `Some(())`.
    ///
    /// Or return `None` if `first` or `last` does not belong to `self`.
    ///
    /// # Safety
    ///
    ///  * `first`, `last` - `first` must be to the left of the `last` 
    ///    (`first` can be the same node as `last`) and
    ///    __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    ///
    /// Must be called after obtained a write lock of `self.rwlock`.
    #[must_use]
    unsafe fn splice_impl(&mut self, first: &'a Node, last: &'a Node) -> Option<()> {
        use Ordering::Relaxed;

        let prev_node = first.get_prev_ptr().load(Relaxed);
        let next_node = last .get_next_ptr().load(Relaxed);

        let last_ptr = if next_node.is_null() {
            &self.last_ptr
        } else {
            let next_node = next_node as *mut Node;
            (*next_node).get_prev_ptr()
        };
        let last = last as *const _ as *mut ();
        match last_ptr.compare_exchange_weak(last, prev_node, Relaxed, Relaxed) {
            Ok(_) => (),
            Err(_) => return None,
        }

        let first_ptr = if prev_node.is_null() {
            &self.first_ptr
        } else {
            let prev_node = prev_node as *mut Node;
            (*prev_node).get_next_ptr()
        };
        let first = first as *const _ as *mut ();
        if ptr::eq(first, last) {
            assert_store_ptr_relaxed(first_ptr, first, next_node);
        } else {
            match first_ptr.compare_exchange_weak(first, next_node, Relaxed, Relaxed) {
                Ok(_) => (),
                Err(_) => {
                    // Revert the change of last_ptr
                    assert_store_ptr_relaxed(last_ptr, prev_node, last);
                    return None
                },
            }
        }

        Some(())
    }

    /// Move all list nodes between `first` and `last` (inclusive) from `self`
    /// and return them as `Some(Splice)`.
    ///
    /// Or return `None` if `first` or `last` does not belong to `self`.
    ///
    /// # Safety
    ///
    ///  * `first`, `last` - `first` must be to the left of the `last` and
    ///    __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    #[must_use]
    pub unsafe fn splice(
        &mut self,
        first: &'a Node,
        last: &'a Node
    ) -> Option<Splice<'a, Node>> {
        self.splice_impl(first, last).map(|_| {Splice::new_unchecked(first, last)})
    }
}

/// `Splice` can be used to
///  - move list nodes between `IntrusiveList` efficiently;
///  - insert/remove list nodes from one `IntrusiveList` efficiently;
///  - iterate over list and remove nodes without starving the rest of readers/removers
///    waiting on `IntrusiveList`.
pub struct Splice<'a, Node: IntrusiveListNode<'a>> {
    first_ptr: * mut (),
    last_ptr: *mut (),
    phantom: PhantomData<&'a Node>,
}
unsafe impl<'a, Node: IntrusiveListNode<'a> + Debug> Send for Splice<'a, Node> {}
impl<'a, Node: IntrusiveListNode<'a> + Debug> Debug for Splice<'a, Node> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.debug_list().entries(self).finish()
    }
}
impl<'a, Node: IntrusiveListNode<'a>> Default for Splice<'a, Node> {
    fn default() -> Self {
        Self::new_empty()
    }
}
impl<'a, Node: IntrusiveListNode<'a>> Splice<'a, Node> {
    /// # Safety
    ///
    /// Assumes `first` and `last` is already linked, `first` must be to the
    /// left of the `last` (`first` and `last` can be the same node)
    /// and and the link must not be modified after `Splice` is created.
    ///
    /// Also, __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    /// ADD IT TO THE SAME LIST SIMULTANEOUSLY
    /// but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    pub unsafe fn new_unchecked(first: &'a Node, last: &'a Node) -> Self {
        Self {
            first_ptr: first as *const _ as *mut (),
            last_ptr:  last  as *const _ as *mut (),
            phantom: PhantomData,
        }
    }

    pub fn new_empty() -> Self {
        let null = ptr::null_mut();
        Self {
            first_ptr: null,
            last_ptr:  null,
            phantom: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.first_ptr.is_null()
    }

    /// # Safety
    ///
    ///  * `node` -  __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    pub unsafe fn push_front(&mut self, node: &'a Node) {
        self.push_front_splice(Splice::new_unchecked(node, node))
    }

    pub fn push_front_splice(&mut self, splice: Self) {
        use Ordering::Relaxed;

        let last_node = unsafe { &*(splice.last_ptr as *mut Node as *const Node) };

        let first = self.first_ptr;

        last_node.get_next_ptr().store(first, Relaxed);

        self.first_ptr = splice.first_ptr;
        if first.is_null() {
            self.last_ptr = splice.last_ptr;
        } else {
            let first = unsafe { &*(first as *mut Node as *const Node) };
            first.get_prev_ptr().store(splice.last_ptr, Relaxed);
        }
    }

    /// # Safety
    ///
    ///  * `node` -  __**YOU MUST NOT USE IT IN OTHER LISTS/SPLICES SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    pub unsafe fn push_back(&mut self, node: &'a Node) {
        self.push_back_splice(Splice::new_unchecked(node, node))
    }

    pub fn push_back_splice(&mut self, splice: Self) {
        use Ordering::Relaxed;

        let first_node = unsafe { &*(splice.first_ptr as *mut Node as *const Node) };

        let last = self.last_ptr;

        first_node.get_prev_ptr().store(last, Relaxed);

        self.last_ptr = splice.last_ptr;
        if last.is_null() {
            self.first_ptr = splice.first_ptr;
        } else {
            let last = unsafe { &*(last as *mut Node as *const Node) };
            last.get_next_ptr().store(splice.first_ptr, Relaxed);
        }
    }

    pub fn iter(&self) -> IntrusiveListIterator<'a, '_, Node> {
        IntrusiveListIterator::from_splice(self)
    }
}
impl<'a, Node: IntrusiveListNode<'a>>
    From<Splice<'a, Node>> for Option<(&'a Node, &'a Node)>
{
    /// If `splice` is empty, then return value will be `None`.
    fn from(splice: Splice<'a, Node>) -> Self {
        if splice.is_empty() {
            None
        } else {
            Some(unsafe {(
                &* (splice.first_ptr as *mut Node as *const Node),
                &* (splice.last_ptr  as *mut Node as *const Node),
            )})
        }
    }
}

impl<'a, 'b, Node: IntrusiveListNode<'a>> IntoIterator for &'b Splice<'a, Node> {
    type Item = &'a Node;
    type IntoIter = IntrusiveListIterator<'a, 'b, Node>;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter::from_splice(self)
    }
}

impl<'a, 'b, Node: IntrusiveListNode<'a>> IntoIterator for &'b IntrusiveList<'a, Node> {
    type Item = &'a Node;
    type IntoIter = IntrusiveListIterator<'a, 'b, Node>;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter::from_list(self)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct IntrusiveListIterator<'a, 'b, Node: IntrusiveListNode<'a>> {
    first_ptr: * mut (),
    last_ptr: *mut (),
    phantom0: PhantomData<&'a Node>,
    phantom1: PhantomData<&'b ()>,
}
impl<'a, 'b, Node: IntrusiveListNode<'a>> IntrusiveListIterator<'a, 'b, Node> {
    pub(crate) fn from_list(list: &'b IntrusiveList<'a, Node>) -> Self {
        loop {
            let first_ptr = list.first_ptr.load(R_ORD);
            let last_ptr  = list.last_ptr .load(R_ORD);

            if (first_ptr.is_null() && last_ptr.is_null()) ||
               ( (!first_ptr.is_null()) && (!last_ptr.is_null()) )
            {
                break Self {
                    first_ptr,
                    last_ptr,
                    phantom0: PhantomData,
                    phantom1: PhantomData,
                }
            }
        }
    }

    pub(crate) fn from_splice(splice: &'b Splice<'a, Node>) -> Self {
        Self {
            first_ptr: splice.first_ptr,
            last_ptr:  splice.last_ptr,
            phantom0: PhantomData,
            phantom1: PhantomData,
        }
    }
}

impl<'a, 'b, Node: IntrusiveListNode<'a>>
    Iterator for IntrusiveListIterator<'a, 'b, Node>
{
    type Item = &'a Node;

    fn next(&mut self) -> Option<Self::Item> {
        if self.first_ptr.is_null() {
            return None;
        }

        let curr_node = unsafe { &* (self.first_ptr as *mut Node as *const Node) };

        if self.first_ptr == self.last_ptr {
            self.first_ptr = ptr::null_mut();
            self.last_ptr = self.first_ptr;
        } else {
            self.first_ptr = curr_node.get_next_ptr().load(Ordering::Relaxed);
        }

        Some(curr_node)
    }

    fn last(self) -> Option<Self::Item> {
        if self.last_ptr.is_null() {
            None
        } else {
            Some(unsafe { &* (self.last_ptr as *mut Node as *const Node) })
        }
    }
}
impl<'a, 'b, Node: IntrusiveListNode<'a>>
    DoubleEndedIterator for IntrusiveListIterator<'a, 'b, Node>
{
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.last_ptr.is_null() {
            return None;
        }

        let curr_node = unsafe { &* (self.last_ptr as *mut Node as *const Node) };

        if self.first_ptr == self.last_ptr {
            self.first_ptr = ptr::null_mut();
            self.last_ptr = self.first_ptr;
        } else {
            self.last_ptr = curr_node.get_prev_ptr().load(Ordering::Relaxed);
        }

        Some(curr_node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use more_asserts::assert_lt;

    use concurrency_toolkit::sync::{Arc, RwLock};
    use concurrency_toolkit::{spawn, join, yield_now};

    use once_cell::sync::Lazy;

    type Node<T = usize> = IntrusiveListNodeImpl<T>;

    fn setup() -> Vec<Node> {
        (0..100).map(Node::new).collect()
    }

    #[concurrency_toolkit::test]
    fn test_splice_empty() {
        let splice: Splice<'_, Node> = Default::default();

        assert!(splice.is_empty());
    }

    #[concurrency_toolkit::test]
    fn test_splice_push_back_splice() {
        let nodes = setup();

        let mut splice0: Splice<'_, _> = Default::default();

        for node in &nodes[0..50] {
            unsafe { splice0.push_back(node) };
            assert!(!splice0.is_empty());
        }

        for (index, node) in splice0.iter().enumerate() {
            assert_eq!(index, *node.get_elem());
            assert_lt!(index, 50);
        }

        let mut splice1: Splice<'_, _> = Default::default();

        for node in &nodes[50..100] {
            unsafe { splice1.push_back(node) };
            assert!(!splice1.is_empty());
        }

        for (index, node) in splice1.iter().enumerate() {
            assert_eq!(index + 50, *node.get_elem());
            assert_lt!(index, 50);
        }

        splice0.push_back_splice(splice1);

        for (index, node) in splice0.iter().enumerate() {
            assert_eq!(index, *node.get_elem());
            assert_lt!(index, 100);
        }
    }

    #[concurrency_toolkit::test]
    fn test_splice_push_back_and_push_front() {
        let nodes = setup();

        let mut splice: Splice<'_, _> = Default::default();

        // Test push_back + push_front + next
        for node in &nodes {
            if *node.get_elem() % 2 == 0 {
                unsafe { splice.push_back(node) };
            } else {
                unsafe { splice.push_front(node) };
            }

            assert!(!splice.is_empty());
        }

        let mut iter = splice.iter();

        for (index, node) in (1..100).rev().step_by(2).zip(&mut iter) {
            assert_lt!(index, 100);
            assert_eq!(index % 2, 1);
            assert_eq!(index, *node.get_elem());
        }
        assert!(!splice.is_empty());

        for (index, node) in (0..100).step_by(2).zip(iter) {
            assert_lt!(index, 100);
            assert_eq!(index % 2, 0);
            assert_eq!(index, *node.get_elem());
        }
    }

    #[concurrency_toolkit::test]
    fn test_splice_push_back_splice_and_push_front_splice() {
        let nodes = setup();

        let mut splice0: Splice<'_, _> = Default::default();
        let mut splice1: Splice<'_, _> = Default::default();

        for node in &nodes {
            if *node.get_elem() % 2 == 0 {
                unsafe { splice0.push_back(node) };
                assert!(!splice0.is_empty());
            } else {
                unsafe { splice1.push_front(node) };
                assert!(!splice1.is_empty());
            }
        }

        let mut splice:  Splice<'_, _> = Default::default();
        splice.push_back_splice(splice0);
        splice.push_front_splice(splice1);

        let mut iter = splice.iter();

        for (index, node) in (1..100).rev().step_by(2).zip(&mut iter) {
            assert_lt!(index, 100);
            assert_eq!(index % 2, 1);
            assert_eq!(index, *node.get_elem());
        }
        assert!(!splice.is_empty());

        for (index, node) in (0..100).step_by(2).zip(iter) {
            assert_lt!(index, 100);
            assert_eq!(index % 2, 0);
            assert_eq!(index, *node.get_elem());
        }
    }

    #[concurrency_toolkit::test]
    fn test_iterator() {
        let nodes = setup();

        let mut splice: Splice<'_, _> = Default::default();
 
        assert_matches!(splice.iter().next(), None);
        assert_matches!(splice.iter().last(), None);
        assert_matches!(splice.iter().next_back(), None);

        for node in &nodes {
            unsafe { splice.push_back(node) };
            assert!(!splice.is_empty());
        }

        assert_matches!(splice.iter().next(), Some(node) if *node.get_elem() == 0);
        assert_matches!(splice.iter().last(), Some(node) if *node.get_elem() == 99);
        assert_matches!(splice.iter().next_back(), Some(node) if *node.get_elem() == 99);

        for (node, index) in splice.iter().rev().zip((0..100).rev()) {
            assert_eq!(index, *node.get_elem());
        }
    }

    #[concurrency_toolkit::test]
    fn test_list_push_back_splice() {
        let nodes = setup();

        let list = IntrusiveList::new();
        eprintln!("list = {:#?}", list);

        for node in &nodes[0..50] {
            unsafe { list.push_back(node) };
            assert!(!list.is_empty());
        }

        for (index, node) in list.iter().enumerate() {
            assert_eq!(index, *node.get_elem());
            assert_lt!(index, 50);
        }

        let mut splice: Splice<'_, _> = Default::default();

        for node in &nodes[50..100] {
            unsafe { splice.push_back(node) };
            assert!(!splice.is_empty());
        }

        for (index, node) in splice.iter().enumerate() {
            assert_eq!(index + 50, *node.get_elem());
            assert_lt!(index, 50);
        }

        list.push_back_splice(splice);

        for (index, node) in list.iter().enumerate() {
            assert_eq!(index, *node.get_elem());
            assert_lt!(index, 100);
        }
    }

    #[concurrency_toolkit::test]
    fn test_list_push_back_splice_and_push_front_splice() {
        let nodes = setup();

        let mut splice0: Splice<'_, _> = Default::default();
        let mut splice1: Splice<'_, _> = Default::default();

        for node in &nodes {
            if *node.get_elem() % 2 == 0 {
                unsafe { splice0.push_back(node) };
                assert!(!splice0.is_empty());
            } else {
                unsafe { splice1.push_front(node) };
                assert!(!splice1.is_empty());
            }
        }

        let list = IntrusiveList::new();
        list.push_back_splice(splice0);
        list.push_front_splice(splice1);
        assert!(!list.is_empty());

        let mut iter = list.iter();

        for (index, node) in (1..100).rev().step_by(2).zip(&mut iter) {
            assert_lt!(index, 100);
            assert_eq!(index % 2, 1);
            assert_eq!(index, *node.get_elem());
        }
        assert!(!list.is_empty());

        for (index, node) in (0..100).step_by(2).zip(iter) {
            assert_lt!(index, 100);
            assert_eq!(index % 2, 0);
            assert_eq!(index, *node.get_elem());
        }
    }

    #[concurrency_toolkit::test]
    fn test_list_clear() {
        let nodes = setup();

        let mut list = IntrusiveList::new();

        for node in &nodes[0..50] {
            unsafe { list.push_back(node) };
        }
        assert!(!list.is_empty());

        list.clear();
        assert!(list.is_empty());

        assert_eq!("[]", format!("{:#?}", list));
    }

    #[concurrency_toolkit::test]
    fn test_list_splice() {
        let nodes = setup();

        let mut list = IntrusiveList::new();

        for node in &nodes {
            unsafe { list.push_back(node) };
        }
        assert!(!list.is_empty());

        let first = list.iter().nth(50).unwrap();
        let last  = list.iter().last().unwrap();

        // TODO: Failed in `unwrap()` when testing under miri
        for (index, node) in unsafe {
            list.splice(first, last).unwrap()
        }.iter().enumerate() {
            assert_eq!(index + 50, *node.get_elem());
            assert_lt!(index, 50);
        }

        for (index, node) in list.iter().enumerate() {
            assert_eq!(index, *node.get_elem());
            assert_lt!(index, 50);
        }
    }

    #[concurrency_toolkit::test]
    fn test_list_remove_if() {
        let nodes = setup();

        let mut list = IntrusiveList::new();

        for node in &nodes {
            unsafe { list.push_back(node) };
        }
        assert!(!list.is_empty());

        assert_eq!((50, 50), list.remove_if(|node| *node.get_elem() % 2 == 1));

        for (index, node) in (0..100).step_by(2).zip(&list) {
            assert_eq!(index, *node.get_elem());
        }
    }

    #[concurrency_toolkit::test]
    fn test_list_push_back_splice_concurrent() {
        static NODES: Lazy<Vec<Node>> = Lazy::new(setup);
        static LIST: Lazy<IntrusiveList<'static, Node>> = Lazy::new(IntrusiveList::new);

        let mut splice0: Splice<'_, _> = Default::default();
        let mut splice1: Splice<'_, _> = Default::default();

        let nodes = &*NODES;

        for node in &nodes[0..50] {
            unsafe { splice0.push_back(node) };
        }

        for node in &nodes[50..100] {
            unsafe { splice1.push_back(node) };
        }

        let handle0 = spawn!({
            LIST.push_back_splice(splice0);
        });

        let handle1 = spawn!({
            LIST.push_back_splice(splice1);
        });

        join!(handle0);
        join!(handle1);
    }
}
