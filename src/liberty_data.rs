use std::ops::Deref;
use std::sync::Arc;

/// An owned view of immutable Liberty data. Child views retain their parent
/// and address a field or an index within it.
pub(crate) struct Shared<T>(Arc<dyn View<T>>);

trait View<T>: Send + Sync {
    fn get(&self) -> &T;
}

struct Root<T>(T);

impl<T: Send + Sync> View<T> for Root<T> {
    fn get(&self) -> &T {
        &self.0
    }
}

struct Child<P, F> {
    parent: Shared<P>,
    select: F,
}

impl<P, T, F> View<T> for Child<P, F>
where
    F: Fn(&P) -> &T + Send + Sync,
{
    fn get(&self) -> &T {
        (self.select)(&self.parent)
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_ref().get()
    }
}

impl<T: 'static> Shared<T> {
    pub(crate) fn new(value: T) -> Self
    where
        T: Send + Sync,
    {
        Self(Arc::new(Root(value)))
    }

    pub(crate) fn project<U>(
        &self,
        select: impl Fn(&T) -> &U + Send + Sync + 'static,
    ) -> Shared<U> {
        Shared(Arc::new(Child {
            parent: self.clone(),
            select,
        }))
    }

    pub(crate) fn child<U: 'static>(&self, field: fn(&T) -> &Vec<U>, index: usize) -> Shared<U> {
        self.project(move |parent| &field(parent)[index])
    }

    pub(crate) fn children<U: 'static>(
        &self,
        field: fn(&T) -> &Vec<U>,
    ) -> impl Iterator<Item = Shared<U>> {
        (0..field(self).len()).map(move |index| self.child(field, index))
    }
}
