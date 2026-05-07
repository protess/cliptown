#[test]
fn fake_clock_is_deterministic() {
    use cliptown_world::det::testing::*;
    let c = FakeClock::at(100);
    assert_eq!(<FakeClock as cliptown_world::det::Clock>::now_unix(&c), 100);
    c.advance(5);
    assert_eq!(<FakeClock as cliptown_world::det::Clock>::now_unix(&c), 105);
}
