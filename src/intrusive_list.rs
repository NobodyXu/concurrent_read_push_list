use core::marker::PhantomData;
use core::ptr;

use concurrency_toolkit::{sync::RwLock, obtain_read_lock, obtain_write_lock};

pub use core::sync::atomic::{AtomicPtr, Ordering};

/// Doubly linked intrusive list node.
///
/// **`self.get_next_ptr()` and `self.get_prev_ptr()` must return different pointers.**
///
/// `T` can either be an immutable reference or a `Sized` object, it is not recommended
/// to return a mutable reference.
pub trait IntrusiveListNode<T> {
    fn get_next_ptr(&self) -> &AtomicPtr<()>;
    fn get_prev_ptr(&self) -> &AtomicPtr<()>;

    fn get_elem(&self) -> T;
}

/// IntrusiveList guarantees that
///  - push and read can be done concurrently while allowing stale read;
///  - deletion can only be done sequentially when there is no
///    writer (excluding the thread doing deletion) or reader.
pub struct IntrusiveList<'a, Node: IntrusiveListNode<T>, T> {
    first_ptr: AtomicPtr<Node>,
    last_ptr: AtomicPtr<Node>,
    rwlock: RwLock<()>,
    phantom0: PhantomData<T>,
    phantom1: PhantomData<&'a Node>,
}
impl<'a, Node: IntrusiveListNode<T>, T> Default for IntrusiveList<'a, Node, T> {
    fn default() -> Self {
        Self::new()
    }
}
const RW_ORD: Ordering = Ordering::AcqRel;
const R_ORD: Ordering = Ordering::Acquire;
const W_ORD: Ordering = Ordering::Release;

impl<'a, Node: IntrusiveListNode<T>, T> IntrusiveList<'a, Node, T> {
    pub fn new() -> Self {
        Self {
            first_ptr: AtomicPtr::new(ptr::null_mut()),
            last_ptr: AtomicPtr::new(ptr::null_mut()),
            rwlock: RwLock::new(()),
            phantom0: PhantomData,
            phantom1: PhantomData,
        }
    }

    /// # Safety
    ///
    /// * `node` - it must not be added twice!
    pub async unsafe fn push_back(&self, node: &'a Node) {
        let _read_guard = obtain_read_lock!(&self.rwlock);
        let null = ptr::null_mut();

        loop {
            let last = self.last_ptr.load(R_ORD);

            node.get_next_ptr().store(null, W_ORD);
            node.get_prev_ptr().store(last as *mut (), W_ORD);

            if last.is_null() {
                let null = ptr::null_mut();
                let node = node as *const Node as *mut Node;
                match self.first_ptr.compare_exchange_weak(null, node, RW_ORD, R_ORD) {
                    Ok(_) => (),
                    Err(_) => continue,
                }
                assert!(ptr::eq(null, self.last_ptr.swap(node, RW_ORD)));
            } else {
                let last = &*(last as *const Node);
                let node = node as *const Node as *mut Node as *mut ();
                match last
                    .get_next_ptr()
                    .compare_exchange_weak(null, node, RW_ORD, R_ORD)
                {
                    Ok(_) => (),
                    Err(_) => continue,
                }
                let node = node as *mut Node;
                assert!(ptr::eq(last, self.last_ptr.swap(node, RW_ORD)));
            }
        }
    }

    /// # Safety
    ///
    /// * `node` - it must not be added twice!
    pub async unsafe fn push_front(&self, node: &'a Node) {
        let _read_guard = obtain_read_lock!(&self.rwlock);
        let null = ptr::null_mut();

        loop {
            let first = self.first_ptr.load(R_ORD);

            node.get_next_ptr().store(first as *mut (), W_ORD);
            node.get_prev_ptr().store(null, W_ORD);

            if first.is_null() {
                let null = ptr::null_mut();
                let node = node as *const Node as *mut Node;
                match self.first_ptr.compare_exchange_weak(null, node, RW_ORD, R_ORD) {
                    Ok(_) => (),
                    Err(_) => continue,
                }
                assert!(ptr::eq(null, self.last_ptr.swap(node, RW_ORD)));
            } else {
                let first = &*(first as *const Node);
                let node = node as *const Node as *mut Node as *mut ();
                match first
                    .get_prev_ptr()
                    .compare_exchange_weak(null, node, RW_ORD, R_ORD)
                {
                    Ok(_) => (),
                    Err(_) => continue,
                }
                let node = node as *mut Node;
                assert!(ptr::eq(first, self.first_ptr.swap(node, RW_ORD)));
            }
        }
    }
}