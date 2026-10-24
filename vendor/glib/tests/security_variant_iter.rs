use glib::variant::ToVariant;

fn strings() -> glib::Variant {
    ["alpha", "", "λ", "🦀", "omega"].to_variant()
}

#[test]
fn next_reads_each_string_in_optimized_builds() {
    let value = strings();
    assert_eq!(
        value.array_iter_str().unwrap().collect::<Vec<_>>(),
        ["alpha", "", "λ", "🦀", "omega"]
    );
}

#[test]
fn nth_and_last_read_the_expected_strings() {
    let value = strings();
    assert_eq!(value.array_iter_str().unwrap().nth(2), Some("λ"));
    assert_eq!(value.array_iter_str().unwrap().last(), Some("omega"));
}

#[test]
fn next_back_and_nth_back_read_the_expected_strings() {
    let value = strings();
    assert_eq!(value.array_iter_str().unwrap().next_back(), Some("omega"));
    assert_eq!(value.array_iter_str().unwrap().nth_back(3), Some(""));
}

#[test]
fn mixed_directions_keep_bounds_and_borrowed_strings() {
    let value = strings();
    let mut iter = value.array_iter_str().unwrap();
    let first = iter.next().unwrap();
    assert_eq!(iter.next_back(), Some("omega"));
    assert_eq!(iter.nth(1), Some("λ"));
    assert_eq!(iter.next_back(), Some("🦀"));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
    assert_eq!(first, "alpha");
}

#[test]
fn empty_arrays_and_skips_are_exhausted() {
    let empty: [&str; 0] = [];
    let value = empty.to_variant();
    let mut iter = value.array_iter_str().unwrap();
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
    assert_eq!(strings().array_iter_str().unwrap().nth(99), None);
    assert_eq!(strings().array_iter_str().unwrap().nth_back(99), None);
}
