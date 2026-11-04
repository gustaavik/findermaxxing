use domain::StoragePath;

/// Per-tab back/forward navigation stack.
#[derive(Debug, Default)]
pub(crate) struct History {
    back: Vec<StoragePath>,
    current: Option<StoragePath>,
    forward: Vec<StoragePath>,
}

impl History {
    /// Navigating to a new place discards the forward stack, like a browser.
    pub(crate) fn visit(&mut self, location: StoragePath) {
        if self.current.as_ref() == Some(&location) {
            return;
        }
        if let Some(previous) = self.current.replace(location) {
            self.back.push(previous);
        }
        self.forward.clear();
    }

    pub(crate) fn go_back(&mut self) -> Option<&StoragePath> {
        let target = self.back.pop()?;
        if let Some(current) = self.current.replace(target) {
            self.forward.push(current);
        }
        self.current.as_ref()
    }

    pub(crate) fn go_forward(&mut self) -> Option<&StoragePath> {
        let target = self.forward.pop()?;
        if let Some(current) = self.current.replace(target) {
            self.back.push(current);
        }
        self.current.as_ref()
    }

    pub(crate) fn current(&self) -> Option<&StoragePath> {
        self.current.as_ref()
    }

    pub(crate) fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub(crate) fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> StoragePath {
        StoragePath::parse(&format!("file://{p}")).unwrap()
    }

    #[test]
    fn back_and_forward_walk_the_stack() {
        let mut history = History::default();
        history.visit(path("/a"));
        history.visit(path("/b"));
        history.visit(path("/c"));
        assert_eq!(history.go_back(), Some(&path("/b")));
        assert_eq!(history.go_back(), Some(&path("/a")));
        assert_eq!(history.go_back(), None);
        assert_eq!(history.go_forward(), Some(&path("/b")));
        assert!(history.can_go_back() && history.can_go_forward());
    }

    #[test]
    fn visiting_clears_forward_and_ignores_repeats() {
        let mut history = History::default();
        history.visit(path("/a"));
        history.visit(path("/b"));
        history.go_back();
        history.visit(path("/c"));
        assert!(!history.can_go_forward());
        history.visit(path("/c"));
        assert_eq!(history.go_back(), Some(&path("/a")));
    }
}
