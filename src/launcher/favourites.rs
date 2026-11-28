//! Favourites: result ids the palette lists first, in the user's order, while the search
//! field is empty. Kept in `LauncherConfig::favourites`; ids whose item is gone (an app
//! uninstalled, a quicklink deleted) or hidden stay listed there but aren't shown, so the
//! favourite comes back with the item.

/// Whether `id` can be a favourite: anything with a stable id, so not calculations or
/// volume levels, whose ids embed what was typed.
pub(crate) fn can_favourite(id: &str) -> bool {
    !id.is_empty() && !id.starts_with("calc:") && !id.starts_with("volume:")
}

/// Adds `id` at the end, or removes it if it's already there. Returns whether it's a
/// favourite now.
pub(crate) fn toggle(favourites: &mut Vec<String>, id: &str) -> bool {
    if favourites.iter().any(|f| f == id) {
        favourites.retain(|f| f != id);
        false
    } else {
        favourites.push(id.to_owned());
        true
    }
}

/// Moves `id` `offset` places among the favourites that are `shown`, so every press moves
/// it past one the user can see. Hidden ones keep their places. Returns whether it moved.
pub(crate) fn move_by(
    favourites: &mut Vec<String>,
    id: &str,
    offset: i32,
    shown: impl Fn(&str) -> bool,
) -> bool {
    let positions: Vec<usize> = favourites
        .iter()
        .enumerate()
        .filter(|(_, f)| *f == id || shown(f))
        .map(|(i, _)| i)
        .collect();
    let Some(from) = positions.iter().position(|&i| favourites[i] == id) else {
        return false;
    };
    let last = positions.len() as i64 - 1;
    let to = (from as i64 + i64::from(offset)).clamp(0, last) as usize;
    if to == from {
        return false;
    }
    let removed_at = positions[from];
    let item = favourites.remove(removed_at);
    let target = positions[to] - usize::from(positions[to] > removed_at);
    favourites.insert(if to > from { target + 1 } else { target }, item);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|&id| id.to_owned()).collect()
    }

    #[test]
    fn toggles_in_and_out() {
        let mut favourites = list(&["a"]);
        assert!(toggle(&mut favourites, "b"));
        assert_eq!(favourites, list(&["a", "b"]));
        assert!(!toggle(&mut favourites, "a"));
        assert_eq!(favourites, list(&["b"]));
    }

    #[test]
    fn only_stable_ids() {
        assert!(can_favourite("app:com.apple.Safari"));
        assert!(can_favourite("snippet:1"));
        assert!(!can_favourite("calc:4"));
        assert!(!can_favourite("volume:50"));
        assert!(!can_favourite(""));
    }

    #[test]
    fn moves_up_and_down() {
        let mut favourites = list(&["a", "b", "c"]);
        assert!(move_by(&mut favourites, "a", 1, |_| true));
        assert_eq!(favourites, list(&["b", "a", "c"]));
        assert!(move_by(&mut favourites, "c", -1, |_| true));
        assert_eq!(favourites, list(&["b", "c", "a"]));
        assert!(move_by(&mut favourites, "a", -2, |_| true));
        assert_eq!(favourites, list(&["a", "b", "c"]));
    }

    #[test]
    fn stops_at_the_ends() {
        let mut favourites = list(&["a", "b"]);
        assert!(!move_by(&mut favourites, "a", -1, |_| true));
        assert!(!move_by(&mut favourites, "b", 1, |_| true));
        assert!(!move_by(&mut favourites, "missing", 1, |_| true));
        assert_eq!(favourites, list(&["a", "b"]));
    }

    #[test]
    fn moves_past_hidden_ones() {
        // "x" is hidden (uninstalled): moving "a" down passes "b", the next one shown.
        let mut favourites = list(&["a", "x", "b", "c"]);
        assert!(move_by(&mut favourites, "a", 1, |id| id != "x"));
        assert_eq!(favourites, list(&["x", "b", "a", "c"]));
        assert!(move_by(&mut favourites, "a", -1, |id| id != "x"));
        assert_eq!(favourites, list(&["x", "a", "b", "c"]));
    }
}
