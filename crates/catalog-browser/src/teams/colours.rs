//! Each team's colour in the part list: given once, when the team joins
//! the list, from a small fixed palette ([`TEAM_COLOURS`]), and saved with
//! the team so it never changes. A team's mark always shows its code as
//! text too, so two teams with one colour are still told apart.

use desktop_contract::TEAM_COLOURS;

/// The colour for a team joining a list whose teams have `taken`: the
/// first colour of the palette fewest teams have, so the first eight teams
/// all differ.
pub(super) fn next_colour<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: Vec<&str> = taken.into_iter().collect();
    let uses = |colour: &&str| taken.iter().filter(|used| **used == *colour).count();
    TEAM_COLOURS
        .iter()
        .min_by_key(|colour| uses(colour))
        .copied()
        .unwrap_or(TEAM_COLOURS[0])
        .to_owned()
}

/// Whether `colour` is one of the palette's, so a list file edited by hand
/// can never put an unknown colour on screen.
pub(super) fn is_known(colour: &str) -> bool {
    TEAM_COLOURS.contains(&colour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_teams_get_the_first_colour_no_team_has() {
        assert_eq!(next_colour([]), "blue");
        assert_eq!(next_colour(["blue"]), "green");
        // A colour freed by a removed team is given again, first.
        assert_eq!(next_colour(["blue", "purple"]), "green");
    }

    #[test]
    fn past_eight_teams_colours_repeat_evenly() {
        let mut taken: Vec<String> = Vec::new();
        for _ in 0..TEAM_COLOURS.len() {
            let next = next_colour(taken.iter().map(String::as_str));
            taken.push(next);
        }
        assert_eq!(taken, TEAM_COLOURS);
        assert_eq!(next_colour(taken.iter().map(String::as_str)), "blue");
        taken.push("blue".into());
        assert_eq!(next_colour(taken.iter().map(String::as_str)), "green");
    }

    #[test]
    fn only_palette_colours_are_known() {
        assert!(is_known("teal"));
        assert!(!is_known("") && !is_known("#ff0000"));
    }
}
