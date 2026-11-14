//! Paging the bubble between the busy agents (C# Core\Companion.cs `CompanionCarousel`, the Mac's CompanionCarousel.swift; plan §1.7):
//! the pinned page while its agent is busy, else the one the pet follows; the pages wrap; a lone page has no pager. The swipes (36
//! points) and wheel steps (120) that turn them are the overlay's ([`crate::Companion::act`]). Owner: W8-E.

/// `CompanionCarousel.Shown(pinned, active, fallback)`: the pin while its agent is busy, else the fallback.
pub fn shown<'a>(pinned: Option<&'a str>, active: &[String], fallback: Option<&'a str>) -> Option<&'a str> {
    match pinned {
        Some(pin) if active.iter().any(|id| id == pin) => Some(pin),
        _ => fallback,
    }
}

/// `CompanionCarousel.Step(shown, active, offset)`: the page `offset` from the shown one, wrapping; the first (or last) page when the
/// shown one is not among them; None for a single page or no offset.
pub fn step(shown: Option<&str>, active: &[String], offset: i64) -> Option<String> {
    if active.len() <= 1 || offset == 0 {
        return None;
    }
    let count = active.len() as i64;
    match active.iter().position(|id| Some(id.as_str()) == shown) {
        None => Some(if offset > 0 { active[0].clone() } else { active[active.len() - 1].clone() }),
        Some(index) => Some(active[(((index as i64 + offset) % count + count) % count) as usize].clone()),
    }
}

/// `CompanionCarousel.Position(shown, active)`: the shown page's number, from 1.
pub fn position(shown: Option<&str>, active: &[String]) -> Option<usize> {
    active.iter().position(|id| Some(id.as_str()) == shown).map(|index| index + 1)
}
