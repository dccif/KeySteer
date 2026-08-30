//! Internal aggregation for operations that must attempt every cleanup stage.

#[derive(Default)]
pub(crate) struct ErrorBundle {
    failures: Vec<(String, String)>,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl ErrorBundle {
    pub(crate) fn push(&mut self, stage: impl Into<String>, error: impl Into<String>) {
        self.failures.push((stage.into(), error.into()));
    }

    pub(crate) fn record<T>(&mut self, stage: impl Into<String>, result: Result<T, String>) {
        if let Err(error) = result {
            self.push(stage, error);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.failures.is_empty()
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn into_result(self) -> Result<(), String> {
        if self.failures.is_empty() {
            return Ok(());
        }
        let mut failures = self.failures.into_iter();
        let Some((stage, error)) = failures.next() else {
            return Ok(());
        };
        let mut message = format!("{stage}: {error}");
        for (stage, error) in failures {
            message.push_str("; ");
            message.push_str(&stage);
            message.push_str(": ");
            message.push_str(&error);
        }
        Err(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_every_failure_in_stage_order() {
        let mut errors = ErrorBundle::default();
        errors.push("first", "one");
        errors.record::<()>("successful", Ok(()));
        errors.push("second", "two");
        assert_eq!(errors.into_result(), Err("first: one; second: two".into()));
    }
}
