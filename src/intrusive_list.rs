use core::marker::PhantomData;
use core::ptr;
use core::iter::{Iterator, DoubleEndedIterator};
use core::convert::From;

use concurrency_toolkit::maybe_async;
use concurrency_toolkit::sync::RwLock;
use concurrency_toolkit::atomic::{AtomicPtr, Ordering};
use concurrency_toolkit::{obtain_read_lock, obtain_write_lock};

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
/// `node` -  __**YOU MUST NOT USE IT IN TWO LISTS SIMULTANEOUSLY OR
/// ADD IT TO THE SAME LIST SIMULTANEOUSLY
/// but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
pub unsafe trait IntrusiveListNode: IntrusiveForwardListNode {
    fn get_prev_ptr(&self) -> &AtomicPtr<()>;
}

/// Sample implementation of IntrusiveListNode
pub struct IntrusiveListNodeImpl<T: Clone> {
    next_ptr: AtomicPtr<()>,
    prev_ptr: AtomicPtr<()>,
    elem: T,
}
unsafe impl<T: Clone> IntrusiveForwardListNode for IntrusiveListNodeImpl<T> {
    type Target = T;

    fn get_next_ptr(&self) -> &AtomicPtr<()> {
        &self.next_ptr
    }
    fn get_elem(&self) -> Self::Target {
        self.elem.clone()
    }
}
unsafe impl<T: Clone> IntrusiveListNode for IntrusiveListNodeImpl<T> {
    fn get_prev_ptr(&self) -> &AtomicPtr<()> {
        &self.prev_ptr
    }
}

/// IntrusiveList guarantees that
///  - push and read can be done concurrently while allowing stale read;
///  - deletion can only be done sequentially when there is no
///    writer (excluding the thread doing deletion) or reader.
pub struct IntrusiveList<'a, Node: IntrusiveListNode> {
    first_ptr: AtomicPtr<()>,
    last_ptr: AtomicPtr<()>,
    rwlock: RwLock<()>,
    phantom: PhantomData<&'a Node>,
}
impl<'a, Node: IntrusiveListNode> Default for IntrusiveList<'a, Node> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, Node: IntrusiveListNode> IntrusiveList<'a, Node> {
    pub fn new() -> Self {
        Self {
            first_ptr: AtomicPtr::new(ptr::null_mut()),
            last_ptr: AtomicPtr::new(ptr::null_mut()),
            rwlock: RwLock::new(()),
            phantom: PhantomData,
        }
    }

    // TODO: Implements push_*_splice

    /// # Safety
    ///
    ///  * `node` -  __**YOU MUST NOT USE IT IN TWO LISTS SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    #[maybe_async]
    pub async unsafe fn push_back(&self, node: &'a Node) {
        let _read_guard = obtain_read_lock!(&self.rwlock);
        let null = ptr::null_mut();

        node.get_next_ptr().store(null, W_ORD);

        loop {
            let last = self.last_ptr.load(R_ORD);

            node.get_prev_ptr().store(last, W_ORD);

            let node = node as *const _ as *mut ();
            if last.is_null() {
                match self.first_ptr.compare_exchange_weak(null, node, RW_ORD, R_ORD) {
                    Ok(_) => (),
                    Err(_) => continue,
                }
            } else {
                match (*(last as *mut Node))
                    .get_next_ptr()
                    .compare_exchange_weak(null, node, RW_ORD, R_ORD)
                {
                    Ok(_) => (),
                    Err(_) => continue,
                }
            }
            break assert_store_ptr(&self.last_ptr, last, node);
        }
    }

    /// # Safety
    ///
    ///  * `node` -  __**YOU MUST NOT USE IT IN TWO LISTS SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    #[maybe_async]
    pub async unsafe fn push_front(&self, node: &'a Node) {
        let _read_guard = obtain_read_lock!(&self.rwlock);
        let null = ptr::null_mut();

        node.get_prev_ptr().store(null, W_ORD);

        loop {
            let first = self.first_ptr.load(R_ORD);

            node.get_next_ptr().store(first, W_ORD);

            let node = node as *const _ as *mut ();
            if first.is_null() {
                match self.first_ptr.compare_exchange_weak(null, node, RW_ORD, R_ORD) {
                    Ok(_) => break assert_store_ptr(&self.last_ptr, null, node),
                    Err(_) => continue,
                }
            } else {
                match (*(first as *mut Node))
                    .get_prev_ptr()
                    .compare_exchange_weak(null, node, RW_ORD, R_ORD)
                {
                    Ok(_) => break assert_store_ptr(&self.first_ptr, first, node),
                    Err(_) => continue,
                }
            }
        }
    }

    /// Returns `true` if `node` is indeed inside `self`, otherwise `false`.
    ///
    /// # Safety
    ///
    ///  * `node` - it must be in one of the following state:
    ///     - `node.get_next_ptr().is_null() && node.get_prev_ptr().is_null()`
    ///     - `node` is added to `self`
    ///    and, __**YOU MUST NOT USE IT IN TWO LISTS SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    #[maybe_async]
    pub async unsafe fn remove_node(&self, node: &'a Node) -> bool {
        let _write_guard = obtain_write_lock!(&self.rwlock);

        let prev_node = node.get_prev_ptr().load(R_ORD);
        let next_node = node.get_next_ptr().load(R_ORD);

        let node = node as *const _ as *mut _;

        let last_ptr = if next_node.is_null() {
            &self.last_ptr
        } else {
            let next_node = next_node as *mut Node;
            (*next_node).get_prev_ptr()
        };
        match last_ptr.compare_exchange_weak(node, prev_node, RW_ORD, R_ORD) {
            Ok(_) => (),
            Err(_) => return false,
        }

        let first_ptr = if prev_node.is_null() {
            &self.first_ptr
        } else {
            let prev_node = prev_node as *mut Node;
            (*prev_node).get_next_ptr()
        };
        assert_store_ptr(first_ptr, node, next_node);

        true
    }

    /// * `f` - return true to remove the node or false to keep it
    #[maybe_async]
    pub async fn remove_if(&self, mut f: impl FnMut(&'a Node) -> bool) {
        let _write_guard = obtain_write_lock!(&self.rwlock);

        let mut it = self.first_ptr.load(Ordering::Relaxed);

        let mut prev: *const Node = ptr::null();
        let mut beg: *const Node = ptr::null();

        while !it.is_null() {
            let node = unsafe { &* (it as *mut Node as *const Node) };
            if f(node) {
                if beg.is_null() {
                    beg = node;
                }
            } else if !beg.is_null() {
                unsafe { self.splice_impl(&* beg, &* prev).unwrap() };
                beg = ptr::null();
            }
            prev = node;
            it = node.get_next_ptr().load(Ordering::Relaxed);
        }
    }

    #[maybe_async]
    pub async fn clear(&self) {
        let _write_guard = obtain_write_lock!(&self.rwlock);

        let null = ptr::null_mut();

        self.first_ptr.store(null, W_ORD);
        self.last_ptr.store(null, W_ORD);
    }

    /// Move all list nodes between `first` and `last` (inclusive) from `self`
    /// and return `Some(())`.
    ///
    /// Or return `None` if `first` or `last` does not belong to `self`.
    ///
    /// # Safety
    ///
    ///  * `first`, `last` - `first` must be to the left of the `last` and
    ///    __**YOU MUST NOT USE IT IN TWO LISTS SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    ///
    /// Must be called after obtained a write lock of `self.rwlock`.
    #[must_use]
    #[maybe_async]
    async unsafe fn splice_impl(
        &self,
        first: &'a Node,
        last: &'a Node
    ) -> Option<()> {
        let prev_node = first.get_prev_ptr().load(R_ORD);
        let next_node = last.get_next_ptr().load(R_ORD);

        let last_ptr = if next_node.is_null() {
            &self.last_ptr
        } else {
            let next_node = next_node as *mut Node;
            (*next_node).get_prev_ptr()
        };
        let last = last as *const _ as *mut ();
        match last_ptr.compare_exchange_weak(last, prev_node, RW_ORD, R_ORD) {
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
        match first_ptr.compare_exchange_weak(first, next_node, RW_ORD, R_ORD) {
            Ok(_) => (),
            Err(_) => {
                // Revert the change of last_ptr
                assert_store_ptr(last_ptr, prev_node, last);
                return None
            },
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
    ///    __**YOU MUST NOT USE IT IN TWO LISTS SIMULTANEOUSLY OR
    ///    ADD IT TO THE SAME LIST SIMULTANEOUSLY
    ///    but you can REMOVE IT FROM THE SAME LIST SIMULTANEOUSLY**__.
    #[must_use]
    #[maybe_async]
    pub async unsafe fn splice(
        &self,
        first: &'a Node,
        last: &'a Node
    ) -> Option<Splice<'a, Node>> {
        {
            let _write_guard = obtain_write_lock!(&self.rwlock);
            self.splice_impl(first, last)
        }.map(|_| {Splice::new(first, last)})
    }
}
pub struct Splice<'a, Node: IntrusiveListNode> {
    first_ptr: * mut (),
    last_ptr: *mut (),
    phantom: PhantomData<&'a Node>,
}
impl<'a, Node: IntrusiveListNode> Splice<'a, Node> {
    /// # Safety
    ///
    /// Assumes `first` and `last` is already linked, `first` must be to the
    /// left of the `last` and and the link must not be modified
    /// after `Splice` is created.
    pub unsafe fn new(first: &'a Node, last: &'a Node) -> Self {
        Self {
            first_ptr: first as *const _ as *mut (),
            last_ptr:  last  as *const _ as *mut (),
            phantom: PhantomData,
        }
    }
}
impl<'a, Node: IntrusiveListNode> From<Splice<'a, Node>> for (&'a Node, &'a Node) {
    fn from(splice: Splice<'a, Node>) -> Self {
        unsafe {(
            &* (splice.first_ptr as *mut Node as *const Node),
            &* (splice.last_ptr  as *mut Node as *const Node),
        )}
    }
}
impl<'a, Node: IntrusiveListNode> Iterator for Splice<'a, Node> {
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
impl<'a, Node: IntrusiveListNode> DoubleEndedIterator for Splice<'a, Node> {
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
