//! WinRT OCR operation ownership and result conversion.

use super::*;

pub(super) fn stream_system_targets_from_result(
    result: &OcrResult,
    geometry: CaptureGeometry,
    core_bounds: Rect,
    maximum: usize,
    mailbox: &ProviderMailbox,
    started: Instant,
) -> Result<usize, VisionError> {
    let lines = result
        .Lines()
        .map_err(|error| VisionError::Operational(format!("cannot read OCR lines: {error}")))?;
    let count = lines.Size().map_err(|error| {
        VisionError::Operational(format!("cannot read OCR line count: {error}"))
    })?;
    let mut accepted = 0usize;
    let mut batch = Vec::with_capacity(PROVIDER_BATCH_SIZE.min(maximum));
    for index in 0..count {
        if accepted == maximum {
            break;
        }
        let line = lines.GetAt(index).map_err(|error| {
            VisionError::Operational(format!("cannot read OCR line {index}: {error}"))
        })?;
        let words = line
            .Words()
            .map_err(|error| VisionError::Operational(format!("cannot read OCR words: {error}")))?;
        let word_count = words.Size().map_err(|error| {
            VisionError::Operational(format!("cannot read OCR word count: {error}"))
        })?;
        let mut union: Option<Rect> = None;
        for word_index in 0..word_count {
            let native = words
                .GetAt(word_index)
                .and_then(|word| word.BoundingRect())
                .map_err(|error| {
                    VisionError::Operational(format!("cannot read OCR word bounds: {error}"))
                })?;
            let rect = image_to_desktop(
                geometry,
                Rect::new(
                    f64::from(native.X),
                    f64::from(native.Y),
                    f64::from(native.Width),
                    f64::from(native.Height),
                ),
            );
            union = Some(union.map_or(rect, |current| current.union(&rect)));
        }
        let Some(rect) = union.filter(|rect| {
            valid_target_rect(*rect, geometry.desktop_bounds)
                && core_bounds.contains(&rect.center())
        }) else {
            continue;
        };
        // The overlap/core ownership test is intentionally before Text():
        // seam duplicates never allocate a Rust String or UiTarget.
        let mut text = line
            .Text()
            .map_err(|error| {
                VisionError::Operational(format!("cannot read OCR line text: {error}"))
            })?
            .to_string();
        trim_string_in_place(&mut text);
        if text.is_empty() {
            continue;
        }
        batch.push(UiTarget {
            rect,
            name: text,
            role: "static_text".into(),
            native_role: Some("vision:windows-ocr".into()),
        });
        accepted += 1;
        if batch.len() == PROVIDER_BATCH_SIZE {
            mailbox.publish(ProviderEvent::OcrBatch {
                provider: "system",
                elapsed: started.elapsed(),
                targets: std::mem::replace(
                    &mut batch,
                    Vec::with_capacity(PROVIDER_BATCH_SIZE.min(maximum - accepted)),
                ),
            })?;
        }
    }
    if !batch.is_empty() {
        mailbox.publish(ProviderEvent::OcrBatch {
            provider: "system",
            elapsed: started.elapsed(),
            targets: batch,
        })?;
    }
    Ok(accepted)
}

#[must_use = "system OCR operations must be cancelled or closed explicitly"]
pub(super) struct OcrOperationGuard {
    operation: IAsyncOperation<OcrResult>,
    closed: bool,
}

impl OcrOperationGuard {
    pub(super) fn start_notified(
        engine: &OcrEngine,
        bitmap: &windows::Graphics::Imaging::SoftwareBitmap,
        _index: usize,
        notifier: mpsc::SyncSender<SystemOcrInput>,
    ) -> Result<(Self, Arc<SystemOcrCompletion>), String> {
        let operation = engine
            .RecognizeAsync(bitmap)
            .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}"))?;
        let mut guard = Self {
            operation,
            closed: false,
        };
        let completion = Arc::new(SystemOcrCompletion::pending());
        let callback_completion = Arc::clone(&completion);
        if let Err(error) = guard
            .operation
            .SetCompleted(&AsyncOperationCompletedHandler::new(move |_, status| {
                callback_completion.publish(status);
                let _ = notifier.try_send(SystemOcrInput::CompletionWake);
                Ok(())
            }))
        {
            return Err(guard.registration_error(format!(
                "cannot register tiled system OCR completion: {error}"
            )));
        }
        Ok((guard, completion))
    }

    pub(super) fn complete(
        mut self,
        status: SystemOcrCompletionStatus,
    ) -> Result<OcrResult, String> {
        let result = match status {
            SystemOcrCompletionStatus::Completed => self
                .operation
                .GetResults()
                .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}")),
            SystemOcrCompletionStatus::Canceled => Err("system OCR cancelled".into()),
            SystemOcrCompletionStatus::Error => self.operation.ErrorCode().map_or_else(
                |error| Err(format!("cannot read system OCR error: {error}")),
                |error| Err(format!("OcrEngine::RecognizeAsync failed: {error}")),
            ),
            SystemOcrCompletionStatus::NonTerminal => retain_nonterminal_system_ocr_owner(
                "system OCR completion callback reported a non-terminal status",
            ),
        };
        self.finish(result)
    }

    pub(super) fn request_cancel(&self) -> Result<(), String> {
        self.operation
            .Cancel()
            .map_err(|error| format!("cannot cancel system OCR operation: {error}"))
    }

    pub(super) fn close_terminal(mut self) -> Result<(), String> {
        self.finish(Ok(()))
    }

    fn finish<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        let mut cleanup = Vec::new();
        if let Err(error) = self.operation.Close() {
            cleanup.push(format!("cannot close system OCR operation: {error}"));
        }
        self.closed = true;
        combine_result_and_cleanup(result, cleanup)
    }

    fn registration_error(&mut self, error: String) -> String {
        let mut cleanup = self.request_cancel().err().into_iter().collect::<Vec<_>>();
        match self.operation.Status() {
            Ok(AsyncStatus::Completed | AsyncStatus::Canceled | AsyncStatus::Error) => {
                if let Err(error) = self.operation.Close() {
                    cleanup.push(format!("cannot close system OCR operation: {error}"));
                }
                self.closed = true;
            }
            Ok(_) => {
                cleanup.push(
                    "system OCR operation did not reach a terminal state after handler failure"
                        .into(),
                );
                let message = combine_primary_and_cleanup(error, cleanup);
                retain_nonterminal_system_ocr_owner(&message);
            }
            Err(status_error) => {
                cleanup.push(format!(
                    "cannot query system OCR operation after handler failure: {status_error}"
                ));
                let message = combine_primary_and_cleanup(error, cleanup);
                retain_nonterminal_system_ocr_owner(&message);
            }
        }
        combine_primary_and_cleanup(error, cleanup)
    }
}

impl Drop for OcrOperationGuard {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let mut failures = self.request_cancel().err().into_iter().collect::<Vec<_>>();
        match self.operation.Status() {
            Ok(AsyncStatus::Completed | AsyncStatus::Canceled | AsyncStatus::Error) => {
                if let Err(error) = self.operation.Close() {
                    failures.push(format!("cannot close system OCR operation: {error}"));
                }
            }
            Ok(_) => {
                let reason = combine_primary_and_cleanup(
                    "system OCR operation left its explicit owner before reaching a terminal state"
                        .into(),
                    failures,
                );
                retain_nonterminal_system_ocr_owner(&reason);
            }
            Err(error) => {
                failures.push(format!(
                    "cannot query system OCR operation during drop: {error}"
                ));
                let reason = combine_primary_and_cleanup(
                    "system OCR operation status is unknown during owner drop".into(),
                    failures,
                );
                retain_nonterminal_system_ocr_owner(&reason);
            }
        }
        for error in failures {
            crate::support::logging::report_error("windows-vision", error);
        }
    }
}

pub(super) fn combine_result_and_cleanup<T>(
    result: Result<T, String>,
    cleanup: Vec<String>,
) -> Result<T, String> {
    match (result, cleanup.is_empty()) {
        (Ok(value), true) => Ok(value),
        (Ok(_), false) => Err(cleanup.join("; ")),
        (Err(error), _) => Err(combine_primary_and_cleanup(error, cleanup)),
    }
}

pub(super) fn combine_primary_and_cleanup(error: String, cleanup: Vec<String>) -> String {
    if cleanup.is_empty() {
        error
    } else {
        format!("{error}; cleanup: {}", cleanup.join("; "))
    }
}

pub(in crate::platform::windows) fn trim_string_in_place(value: &mut String) {
    let start = value.len() - value.trim_start().len();
    let end = value.trim_end().len();
    value.truncate(end);
    if start != 0 {
        value.drain(..start);
    }
}

pub(in crate::platform::windows) fn image_to_desktop(image: CaptureGeometry, rect: Rect) -> Rect {
    Rect::new(
        image.desktop_bounds.x + rect.x / image.scale,
        image.desktop_bounds.y + rect.y / image.scale,
        rect.width / image.scale,
        rect.height / image.scale,
    )
}

pub(in crate::platform::windows) fn valid_target_rect(rect: Rect, bounds: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width >= 2.0
        && rect.height >= 2.0
        && bounds.intersect(&rect).is_some()
        && !(rect.width >= bounds.width && rect.height >= bounds.height)
}
