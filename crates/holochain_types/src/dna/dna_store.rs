use crate::prelude::*;

/// A readable and writable store of DnaFiles and EntryDefs
#[mockall::automock]
pub trait DnaStore: Default + Send + Sync {
    /// Add a DnaFile to the store
    fn add_dna(&mut self, dna: DnaFile);
    /// Add multiple DnaFiles to the store
    fn add_dnas<T: IntoIterator<Item = (DnaHash, DnaFile)> + 'static>(&mut self, dnas: T);
    /// Add an EntryDef to the store
    fn add_entry_def(&mut self, k: EntryDefBufferKey, entry_def: EntryDef);
    /// Add multiple EntryDefs to the store
    fn add_entry_defs<T: IntoIterator<Item = (EntryDefBufferKey, EntryDef)> + 'static>(
        &mut self,
        entry_defs: T,
    );
    /// List all DNAs in the store
    // TODO: FAST: Make this return an iterator to avoid allocating
    fn list(&self) -> Vec<DnaHash>;
    /// Get a particular DnaDef
    fn get_dna_def(&self, hash: &DnaHash) -> Option<DnaDef>;
    /// Get a particular DnaFile
    fn get_dna_file(&self, hash: &DnaHash) -> Option<DnaFile>;
    /// Get a particular EntryDef
    fn get_entry_def(&self, k: &EntryDefBufferKey) -> Option<EntryDef>;
}

impl MockDnaStore {
    /// Construct the most common mock which has a single DnaFile
    pub fn single_dna<T>(dna_file: DnaFile, add_dnas_calls: T, add_entry_defs_calls: T) -> Self
    where
        T: Into<::mockall::TimesRange>,
    {
        let dna_def = dna_file.dna_def().clone();
        let dna_hash = dna_file.dna_hash().clone();
        let mut dna_store = MockDnaStore::new();

        dna_store
            .expect_get_dna_def()
            .with(mockall::predicate::eq(dna_hash.clone()))
            .returning(move |_| Some(dna_def.clone()));
        dna_store
            .expect_get_dna_file()
            .with(mockall::predicate::eq(dna_hash))
            .returning(move |_| Some(dna_file.clone()));
        dna_store
            .expect_add_dnas::<Vec<_>>()
            .times(add_dnas_calls)
            .return_const(());
        dna_store
            .expect_add_entry_defs::<Vec<_>>()
            .times(add_entry_defs_calls)
            .return_const(());

        dna_store
    }
}

/// Read-only access to a DnaStore, and only for DNAs
pub trait DnaStoreRead: Default + Send + Sync {
    /// List all DNAs in the store
    fn list(&self) -> Vec<DnaHash>;
    /// Get a particular DnaDef
    fn get_dna_def(&self, hash: &DnaHash) -> Option<DnaDef>;
    /// Get a particular DnaFile
    fn get_dna_file(&self, hash: &DnaHash) -> Option<DnaFile>;
}

impl<DS: DnaStore> DnaStoreRead for DS {
    fn list(&self) -> Vec<DnaHash> {
        DS::list(self)
    }

    fn get_dna_def(&self, hash: &DnaHash) -> Option<DnaDef> {
        DS::get_dna_def(self, hash)
    }

    fn get_dna_file(&self, hash: &DnaHash) -> Option<DnaFile> {
        DS::get_dna_file(self, hash)
    }
}

/// Key for the [EntryDef] buffer
#[derive(
    Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize, SerializedBytes,
)]
pub struct EntryDefBufferKey {
    /// The zome to which this entry def belongs
    pub zome: ZomeDef,
    /// The index, for ordering
    pub entry_def_position: EntryDefIndex,
}

impl EntryDefBufferKey {
    /// Create a new key
    pub fn new(zome: ZomeDef, entry_def_position: EntryDefIndex) -> Self {
        Self {
            zome,
            entry_def_position,
        }
    }
}
