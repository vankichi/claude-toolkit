//! Step through a fixed order of values (tabs, panels, sort keys), wrapping
//! at either end.

/// The value after `current` in `order`, wrapping from the last to the first.
/// A `current` not in `order` counts as the first.
///
/// # Panics
/// If `order` is empty.
#[must_use]
pub fn next<T: Copy + PartialEq>(order: &[T], current: T) -> T {
    order[(index(order, &current) + 1) % order.len()]
}

/// The value before `current` in `order`, wrapping from the first to the
/// last. A `current` not in `order` counts as the first.
///
/// # Panics
/// If `order` is empty.
#[must_use]
pub fn prev<T: Copy + PartialEq>(order: &[T], current: T) -> T {
    order[(index(order, &current) + order.len() - 1) % order.len()]
}

fn index<T: PartialEq>(order: &[T], current: &T) -> usize {
    order.iter().position(|v| v == current).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: [char; 3] = ['a', 'b', 'c'];

    #[test]
    fn steps_and_wraps_both_ways() {
        assert_eq!(next(&ORDER, 'a'), 'b');
        assert_eq!(next(&ORDER, 'c'), 'a');
        assert_eq!(prev(&ORDER, 'b'), 'a');
        assert_eq!(prev(&ORDER, 'a'), 'c');
    }

    #[test]
    fn a_value_not_in_the_order_counts_as_the_first() {
        assert_eq!(next(&ORDER, 'z'), 'b');
        assert_eq!(prev(&ORDER, 'z'), 'c');
    }
}
