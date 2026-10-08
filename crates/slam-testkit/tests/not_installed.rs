//! Without the counting allocator the check must fail loudly, not pass silently.

#[test]
#[should_panic(expected = "is not the global allocator")]
fn missing_allocator_is_detected() {
    slam_testkit::assert_no_alloc(|| ());
}
