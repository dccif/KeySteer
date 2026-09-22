//! One ordered transaction barrier around independent confirmation implementations.
use super::history_confirmation::PendingHistory;
use super::layout_confirmation::PendingLayout;
use super::*;
pub(super) enum PendingTransaction {
    Layout(PendingLayout),
    History(PendingHistory),
}
impl PendingTransaction {
    pub fn is_layout(&self) -> bool {
        matches!(self, Self::Layout(_))
    }
    pub fn take_begin(
        session: &mut Session,
        access: &mut impl WindowAccess,
        request: &mut WindowRequest,
        screens: &Arc<[Screen]>,
    ) -> Result<Option<Self>, String> {
        if let Some(layout) = PendingLayout::take_begin(session, access, request, screens)? {
            return Ok(Some(Self::Layout(layout)));
        }
        Ok(PendingHistory::begin(session, access, request, screens)?.map(Self::History))
    }
    pub fn request(&self) -> &WindowRequest {
        match self {
            Self::Layout(work) => &work.request,
            Self::History(work) => &work.request,
        }
    }
    pub fn preparing(&self) -> bool {
        match self {
            Self::Layout(work) => work.preparing(),
            Self::History(work) => work.preparing(),
        }
    }
    pub fn targets(&self) -> impl Iterator<Item = WindowId> + '_ {
        let (layout, history) = match self {
            Self::Layout(work) => (Some(work), None),
            Self::History(work) => (None, Some(work)),
        };
        layout
            .into_iter()
            .flat_map(PendingLayout::targets)
            .chain(history.into_iter().flat_map(PendingHistory::targets))
    }
    pub fn advance_checked(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: &dyn Fn() -> bool,
        now: Instant,
    ) -> Option<WindowResult> {
        match self {
            Self::Layout(work) => work.advance_checked(session, access, cancelled, now),
            Self::History(work) => work.advance(session, access, cancelled, now),
        }
    }
}
