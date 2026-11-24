//! MC-58: a bot's standing instructions. A pane that is a bot (`RunSession.bot`) starts every new conversation (a request that resumes
//! no session) with its instructions after the request, as one paragraph that [`crate::checklist::without_app_notes`] takes off again
//! when the request is read back. The transcript and the request's block keep what the person (or the caller) sent.

use mighty_core::model::PaneBot;

/// How the note starts (the handle and the instructions follow).
pub const NOTE_START: &str = "[Mighty Code] You are the bot @";

/// `input` with the bot's note after it, when the bot has instructions and the request is no slash command; else `input` as it is.
/// The instructions keep their lines but no blank one, so the note stays one paragraph.
pub fn with_note(bot: &PaneBot, input: &str) -> String {
    let Some(instructions) = bot.instructions.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
        return input.to_string();
    };
    if input.trim_start().starts_with('/') {
        return input.to_string();
    }
    let lines: Vec<&str> = instructions.lines().map(str::trim_end).filter(|line| !line.trim().is_empty()).collect();
    format!(
        "{input}\n\n{NOTE_START}{} in Mighty Code: other panes, schedules, the command line and other agents send you requests by that name. Your standing instructions (from the person who set you up; they hold for this whole conversation):\n{}",
        bot.handle,
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::without_app_notes;

    fn reviewer(instructions: Option<&str>) -> PaneBot {
        PaneBot { handle: "reviewer".into(), instructions: instructions.map(str::to_string), auto: None }
    }

    #[test]
    fn the_note_goes_after_the_request_as_one_paragraph_and_comes_off_when_read_back() {
        let sent = with_note(&reviewer(Some("Review diffs.\n\n- Be brief.  \n")), "look at PR 12");
        assert!(sent.starts_with("look at PR 12\n\n[Mighty Code] You are the bot @reviewer"), "{sent}");
        assert!(sent.ends_with("Review diffs.\n- Be brief."), "{sent}");
        assert_eq!(sent.matches("\n\n").count(), 1, "one paragraph: {sent}");
        assert_eq!(without_app_notes(&sent), "look at PR 12");
    }

    #[test]
    fn no_instructions_or_a_slash_command_sends_the_request_as_it_is() {
        assert_eq!(with_note(&reviewer(None), "hi"), "hi");
        assert_eq!(with_note(&reviewer(Some("   ")), "hi"), "hi");
        assert_eq!(with_note(&reviewer(Some("Review.")), "/compact"), "/compact");
    }
}
