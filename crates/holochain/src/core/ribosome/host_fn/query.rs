use crate::core::ribosome::CallContext;
use crate::core::ribosome::HostFnAccess;
use crate::core::ribosome::RibosomeError;
use crate::core::ribosome::RibosomeT;
use holochain_types::prelude::*;
use holochain_wasmer_host::prelude::WasmError;
use std::sync::Arc;

pub fn query(
    _ribosome: Arc<impl RibosomeT>,
    call_context: Arc<CallContext>,
    input: ChainQueryFilter,
) -> Result<Vec<Element>, WasmError> {
    match HostFnAccess::from(&call_context.host_context()) {
        HostFnAccess {
            read_workspace: Permission::Allow,
            ..
        } => tokio_helper::block_forever_on(async move {
            let elements: Vec<Element> = call_context
                .host_context
                .workspace()
                .source_chain()
                .as_ref()
                .expect("Must have source chain to query the source chain")
                .query(input)
                .await
                .map_err(|source_chain_error| WasmError::Host(source_chain_error.to_string()))?;
            Ok(elements)
        }),
        _ => Err(WasmError::Host(
            RibosomeError::HostFnPermissions(
                call_context.zome.zome_name().clone(),
                call_context.function_name().clone(),
                "query".into(),
            )
            .to_string(),
        )),
    }
}

#[cfg(test)]
#[cfg(feature = "slow_tests")]
pub mod slow_tests {
    use hdk::prelude::*;
    use query::ChainQueryFilter;
    use crate::core::ribosome::wasm_test::RibosomeTestFixture;
    use holochain_wasm_test_utils::TestWasm;

    #[tokio::test(flavor = "multi_thread")]
    async fn query_smoke_test() {
        observability::test_run().ok();
        let RibosomeTestFixture {
            conductor, alice, ..
        } = RibosomeTestFixture::new(TestWasm::Query).await;

        let _hash_a: EntryHash = conductor.call(&alice, "add_path", "a".to_string()).await;
        let _hash_b: EntryHash = conductor.call(&alice, "add_path", "b".to_string()).await;

        let elements: Vec<Element> = conductor
            .call(&alice, "query", ChainQueryFilter::default())
            .await;

        assert_eq!(elements.len(), 6);
    }
}
