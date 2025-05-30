use libp2p_mplex::Config;

#[async_std::test]
async fn close_implies_flush() {
    let (alice, bob) =
        libp2p_muxer_test_harness::connected_muxers_on_memory_ring_buffer::<Config, _, _>().await;

    libp2p_muxer_test_harness::close_implies_flush(alice, bob).await;
}

#[async_std::test]
async fn read_after_close() {
    let (alice, bob) =
        libp2p_muxer_test_harness::connected_muxers_on_memory_ring_buffer::<Config, _, _>().await;

    libp2p_muxer_test_harness::read_after_close(alice, bob).await;
}
