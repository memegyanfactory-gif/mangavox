//! Labelled undo history over immutable project snapshots.
//! Ported from bhippi-Video-editor (MIT): 300 steps, drag = preview then one commit, and an AI
//! turn squashed into one `AI: ...` step. Pure and generic; the editor wires it in later.
#![allow(dead_code)]

/// Most steps kept; the oldest fall off.
pub const MAX_STEPS: usize = 300;
/// Label prefix of steps made by the assistant.
pub const AI_PREFIX: &str = "AI: ";

/// What the history needs from a project to highlight an AI turn's changes.
pub trait Diff {
    /// Ids of clips that differ between `self` and `other` (added, removed or changed).
    fn changed_clips(&self, other: &Self) -> Vec<String>;
}

struct Step<P> {
    label: String,
    /// The project as it was after this step.
    state: P,
}

/// Snapshots: `steps[0]` is the baseline (label "Open"), `cursor` the current one.
pub struct History<P: Clone> {
    steps: Vec<Step<P>>,
    cursor: usize,
    /// Live project while dragging; not in `steps` until `settle`.
    preview: Option<P>,
}

impl<P: Clone> History<P> {
    pub fn new(initial: P) -> Self {
        Self { steps: vec![Step { label: "Open".into(), state: initial }], cursor: 0, preview: None }
    }

    /// The project to show: the drag preview if any, else the current snapshot.
    pub fn current(&self) -> &P {
        self.preview.as_ref().unwrap_or(&self.steps[self.cursor].state)
    }

    /// Records `project` as a new step, dropping any redo tail.
    pub fn commit(&mut self, label: &str, project: P) {
        self.preview = None;
        self.steps.truncate(self.cursor + 1);
        self.steps.push(Step { label: label.to_string(), state: project });
        if self.steps.len() > MAX_STEPS {
            self.steps.remove(0);
        }
        self.cursor = self.steps.len() - 1;
    }

    /// A drag in progress: shows `project` without making an undo step.
    pub fn preview(&mut self, project: P) {
        self.preview = Some(project);
    }

    /// The drag ended: one commit for the whole gesture (or nothing if it never moved).
    pub fn settle(&mut self, label: &str) {
        if let Some(p) = self.preview.take() {
            self.commit(label, p);
        }
    }

    /// Abandons a drag.
    pub fn cancel_preview(&mut self) {
        self.preview = None;
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.steps.len()
    }

    /// Steps back; returns the label that was undone.
    pub fn undo(&mut self) -> Option<String> {
        self.preview = None;
        if !self.can_undo() {
            return None;
        }
        let l = self.steps[self.cursor].label.clone();
        self.cursor -= 1;
        Some(l)
    }

    /// Steps forward; returns the label that was redone.
    pub fn redo(&mut self) -> Option<String> {
        self.preview = None;
        if !self.can_redo() {
            return None;
        }
        self.cursor += 1;
        Some(self.steps[self.cursor].label.clone())
    }

    /// Jumps to step `i` (a chat "Jump" card).
    pub fn jump(&mut self, i: usize) -> bool {
        self.preview = None;
        if i >= self.steps.len() {
            return false;
        }
        self.cursor = i;
        true
    }

    /// What Undo would undo.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo().then(|| self.steps[self.cursor].label.as_str())
    }

    /// What Redo would redo.
    pub fn redo_label(&self) -> Option<&str> {
        self.steps.get(self.cursor + 1).map(|s| s.label.as_str())
    }

    /// Index of the current step; use as the `since` mark before an AI turn.
    pub fn mark(&self) -> usize {
        self.cursor
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Labels, oldest first, for a History list.
    pub fn labels(&self) -> Vec<&str> {
        self.steps.iter().map(|s| s.label.as_str()).collect()
    }

    /// Folds the steps after `since` into one step labelled `AI: <label>`. Only steps whose label
    /// starts with "AI: " are folded; if any step in the range is a user step, nothing happens.
    /// Returns whether it squashed.
    pub fn squash_turn(&mut self, since: usize, label: &str) -> bool {
        self.preview = None;
        let end = self.cursor;
        if since >= end || end >= self.steps.len() {
            return false;
        }
        if !self.steps[since + 1..=end].iter().all(|s| s.label.starts_with(AI_PREFIX)) {
            return false;
        }
        let state = self.steps[end].state.clone();
        self.steps.truncate(since + 1);
        let label = if label.starts_with(AI_PREFIX) { label.to_string() } else { format!("{AI_PREFIX}{label}") };
        self.steps.push(Step { label, state });
        self.cursor = self.steps.len() - 1;
        true
    }
}

impl<P: Clone + Diff> History<P> {
    /// Clip ids changed since step `since` (to paint in the AI accent).
    pub fn diff_since(&self, since: usize) -> Vec<String> {
        match self.steps.get(since) {
            Some(s) => s.state.changed_clips(&self.steps[self.cursor].state),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct P(Vec<(String, i32)>);

    impl Diff for P {
        fn changed_clips(&self, o: &Self) -> Vec<String> {
            let mut v: Vec<String> = Vec::new();
            for (id, x) in &self.0 {
                if o.0.iter().find(|(i, _)| i == id).map(|(_, y)| y) != Some(x) {
                    v.push(id.clone());
                }
            }
            for (id, _) in &o.0 {
                if !self.0.iter().any(|(i, _)| i == id) {
                    v.push(id.clone());
                }
            }
            v
        }
    }

    fn p(a: i32) -> P {
        P(vec![("a".into(), a), ("b".into(), 0)])
    }

    #[test]
    fn commit_undo_redo() {
        let mut h = History::new(p(0));
        h.commit("one", p(1));
        h.commit("two", p(2));
        assert_eq!(h.undo().as_deref(), Some("two"));
        assert_eq!(h.current(), &p(1));
        assert_eq!(h.redo().as_deref(), Some("two"));
        h.undo();
        h.commit("three", p(9));
        assert!(!h.can_redo());
        assert_eq!(h.labels(), ["Open", "one", "three"]);
        assert!(h.jump(0) && h.current() == &p(0));
        assert!(!h.jump(7));
    }

    #[test]
    fn drag_is_one_step() {
        let mut h = History::new(p(0));
        for v in 1..=20 {
            h.preview(p(v));
        }
        assert_eq!(h.current(), &p(20));
        h.settle("Move");
        assert_eq!(h.len(), 2);
        h.settle("nothing");
        assert_eq!(h.len(), 2);
        h.preview(p(5));
        h.cancel_preview();
        assert_eq!(h.current(), &p(20));
    }

    #[test]
    fn caps_at_300() {
        let mut h = History::new(p(0));
        for i in 0..400 {
            h.commit("x", p(i));
        }
        assert_eq!(h.len(), MAX_STEPS);
        assert_eq!(h.current(), &p(399));
    }

    #[test]
    fn squash_only_ai_steps() {
        let mut h = History::new(p(0));
        let m = h.mark();
        h.commit("AI: split", p(1));
        h.commit("AI: trim", p(2));
        assert_eq!(h.diff_since(m), ["a"]);
        assert!(h.squash_turn(m, "tighten the intro"));
        assert_eq!(h.labels(), ["Open", "AI: tighten the intro"]);
        assert_eq!(h.current(), &p(2));
        assert_eq!(h.undo_label(), Some("AI: tighten the intro"));
        assert_eq!(h.redo_label(), None);

        // A turn folded as it goes (one step after every tool call) stays one step.
        let m2 = h.mark();
        for v in 10..13 {
            h.commit("AI: edit", p(v));
            assert!(h.squash_turn(m2, "AI: \"x\""));
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.undo().as_deref(), Some("AI: \"x\""));
        assert_eq!(h.current(), &p(2));
        assert_eq!(h.redo_label(), Some("AI: \"x\""));
        assert!(h.redo().is_some());

        let m = h.mark();
        h.commit("AI: a", p(3));
        h.commit("Move clip", p(4));
        assert!(!h.squash_turn(m, "x"));
        assert_eq!(h.len(), 5);
        assert!(!h.squash_turn(h.mark(), "nothing"));
    }
}
