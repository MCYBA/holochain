use crate::buffer::BufferedStore;
use crate::error::DatabaseError;
use crate::error::DatabaseResult;
use crate::prelude::*;
use either::Either;
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::*;

#[cfg(test)]
mod tests;

/// Transactional operations on a KVV store
///
/// Replace is a Delete followed by an Insert
#[derive(Debug, PartialEq, Eq, Clone)]
pub(super) enum KvvOp {
    Insert,
    Delete,
}

#[derive(Clone)]
pub(super) struct ValuesDelta<V> {
    delete_all: bool,
    deltas: BTreeMap<V, KvvOp>,
}

impl<V: Ord + Eq> ValuesDelta<V> {
    fn all_deleted() -> Self {
        Self {
            delete_all: true,
            deltas: BTreeMap::new(),
        }
    }
}

// This would be equivalent to the derived impl, except that this
// doesn't require `V: Default`
impl<V: Ord + Eq> Default for ValuesDelta<V> {
    fn default() -> Self {
        Self {
            delete_all: bool::default(),
            deltas: BTreeMap::new(),
        }
    }
}

/// A persisted key-value store with a transient BTreeMap to store
/// CRUD-like changes without opening a blocking read-write cursor
pub struct KvvBufUsed<K, V>
where
    K: BufKey,
    V: BufMultiVal,
{
    table: MultiTable,
    scratch: BTreeMap<K, ValuesDelta<V>>,
}

impl<K, V> KvvBufUsed<K, V>
where
    K: BufKey + Debug,
    V: BufMultiVal + Debug,
{
    /// Create a new KvvBufUsed
    pub fn new(table: MultiTable) -> Self {
        Self {
            table,
            scratch: BTreeMap::new(),
        }
    }

    /// Get a set of values, taking the scratch space into account,
    /// or from persistence if needed
    #[instrument(skip(self, r))]
    pub fn get<R: Readable, KK: Debug + std::borrow::Borrow<K>>(
        &self,
        r: &mut R,
        k: KK,
    ) -> DatabaseResult<impl Iterator<Item = DatabaseResult<V>>> {
        // Depending on which branches get taken, this function could return
        // any of three different iterator types, in order to unify all three
        // into a single type, we return (in the happy path) a value of type
        // ```
        // Either<__GetPersistedIter, Either<__ScratchSpaceITer, Chain<...>>>
        // ```

        let values_delta: ValuesDelta<V> = if let Some(v) = self.scratch.get(k.borrow()) {
            v.clone()
        } else {
            // Only do the persisted call if it's not in the scratch
            trace!(?k);
            let persisted = self.get_persisted(r, k.borrow())?;

            return Ok(persisted.collect::<Vec<_>>().into_iter());
        };
        let ValuesDelta { delete_all, deltas } = values_delta;

        let from_scratch_space = deltas
            .clone()
            .into_iter()
            .filter(|(_v, op)| *op == KvvOp::Insert)
            .map(|(v, _op)| Ok(v));

        let iter = if delete_all {
            // If delete_all is set, return only scratch content,
            // skipping persisted content (as it will all be deleted)
            Either::Left(from_scratch_space)
        } else {
            let persisted = self.get_persisted(r, k.borrow())?;
            Either::Right(
                from_scratch_space
                    // Otherwise, chain it with the persisted content,
                    // skipping only things that we've specifically deleted or returned.
                    .chain(persisted.filter(move |r| match r {
                        Ok(v) => !deltas.contains_key(v),
                        Err(_e) => true,
                    })),
            )
        };

        Ok(iter.collect::<Vec<_>>().into_iter())
    }

    /// Update the scratch space to record an Insert operation for the KV
    pub fn insert(&mut self, k: K, v: V) {
        self.scratch
            .entry(k)
            .or_default()
            .deltas
            .insert(v, KvvOp::Insert);
    }

    /// Update the scratch space to record a Delete operation for the KV
    pub fn delete(&mut self, k: K, v: V) {
        self.scratch
            .entry(k)
            .or_default()
            .deltas
            .insert(v, KvvOp::Delete);
    }

    /// Clear the scratch space and record a DeleteAll operation
    pub fn delete_all(&mut self, k: K) {
        self.scratch.insert(k, ValuesDelta::all_deleted());
    }

    /// Fetch data from DB, deserialize into V type
    #[instrument(skip(self, r))]
    fn get_persisted<R: Readable>(
        &self,
        r: &mut R,
        k: &K,
    ) -> DatabaseResult<impl Iterator<Item = DatabaseResult<V>>> {
        let s = trace_span!("persisted");
        let _g = s.enter();
        trace!("test");
        let iter = self.table.get_multi(r, k)?;
        Ok(iter.filter_map(|v| match v {
            Ok((_, Some(rusqlite::types::Value::Blob(buf)))) => Some(
                holochain_serialized_bytes::decode(&buf)
                    .map(|n| {
                        trace!(?n);
                        n
                    })
                    .map_err(|e| e.into()),
            ),
            Ok((_, Some(_))) => Some(Err(DatabaseError::InvalidValue)),
            Ok((_, None)) => None,
            Err(e) => Some(Err(e.into())),
        }))
    }

    // TODO: This should be cfg test but can't because it's in a different crate
    /// Clear all scratch and table, useful for tests
    pub fn clear_all(&mut self, writer: &mut Writer) -> DatabaseResult<()> {
        self.scratch.clear();
        Ok(self.table.clear(writer)?)
    }
}

impl<K, V> BufferedStore for KvvBufUsed<K, V>
where
    K: Clone + BufKey + Debug,
    V: BufMultiVal + Debug,
{
    type Error = DatabaseError;

    fn is_clean(&self) -> bool {
        self.scratch.is_empty()
    }

    fn flush_to_txn_ref(&mut self, writer: &mut Writer) -> DatabaseResult<()> {
        use KvvOp::*;
        if self.is_clean() {
            return Ok(());
        }
        for (k, ValuesDelta { delete_all, deltas }) in self.scratch.iter() {
            // If delete_all is set, that we should delete everything persisted,
            // but then continue to add inserts from the ops, if present
            if *delete_all {
                self.table.delete_all(writer, &k)?;
            }
            trace!(?k);
            trace!(?deltas);

            for (v, op) in deltas {
                match op {
                    Insert => {
                        let buf = holochain_serialized_bytes::encode(&v)?;
                        let encoded = rusqlite::types::Value::Blob(buf);

                        self.table.put(writer, &k, &encoded)?;
                    }
                    // Skip deleting unnecessarily if we have already deleted
                    // everything
                    Delete if *delete_all => {}
                    Delete => {
                        let buf = holochain_serialized_bytes::encode(&v)?;
                        let encoded = rusqlite::types::Value::Blob(buf);
                        self.table.delete_kv(writer, &k, &encoded)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Create an KvvBufUsed with a clone of the scratch
/// from another KvvBufUsed
impl<K, V> From<&KvvBufUsed<K, V>> for KvvBufUsed<K, V>
where
    K: BufKey + Debug + Clone,
    V: BufMultiVal + Debug,
{
    fn from(other: &KvvBufUsed<K, V>) -> Self {
        Self {
            table: other.table.clone(),
            scratch: other.scratch.clone(),
        }
    }
}