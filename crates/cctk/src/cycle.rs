//! Step through an order of values (tabs, panels, sort keys, visible rows),
//! wrapping at either end.

/// The value after `current` in `order`, wrapping from the last to the first.
/// A `current` not in `order` counts as the first; an empty `order` returns
/// `current`.
#[must_use]
pub fn next<T: Copy + PartialEq>(order: &[T], current: T) -> T {
    step(order, current, 1)
}

/// The value before `current` in `order`, wrapping from the first to the
/// last. A `current` not in `order` counts as the first; an empty `order`
/// returns `current`.
#[must_use]
pub fn prev<T: Copy + PartialEq>(order: &[T], current: T) -> T {
    step(order, current, order.len().saturating_sub(1))
}

/// `order[(index of current + forward) % len]`; `forward` is 1 for next and
/// `len - 1` for prev, so nothing goes negative.
fn step<T: Copy + PartialEq>(order: &[T], current: T, forward: usize) -> T {
    if order.is_empty() {
        return current;
    }
    let index = order.iter().position(|v| *v == current).unwrap_or(0);
    order[(index + forward) % order.len()]
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

    #[test]
    fn follows_the_given_order_not_the_values() {
        // ccwatch steps through rows in display order, e.g. indices [2, 1, 0].
        let order = [2usize, 1, 0];
        assert_eq!(next(&order, 2), 1);
        assert_eq!(next(&order, 0), 2);
        assert_eq!(prev(&order, 2), 0);
        assert_eq!(prev(&order, 1), 2);
    }

    #[test]
    fn an_empty_order_keeps_current() {
        assert_eq!(next(&[], 3usize), 3);
        assert_eq!(prev(&[], 3usize), 3);
    }
}
