//! Pure membership transitions; the coordinator commits these only after native success.
use crate::api::window::WindowId;
use crate::api::window_tabs::{TabDrop, TabGroup, TabGroupId, TabState, WindowTarget};

#[derive(Clone, Default)]
pub(super) struct Groups {
    pub state: TabState,
    next_number: u32,
}

impl Groups {
    pub fn clear_numbers(&mut self) {
        self.state.numbers.clear();
        self.next_number = 0;
    }
    pub fn same_membership(&self, other: &Self) -> bool {
        self.state.groups.len() == other.state.groups.len()
            && self
                .state
                .groups
                .iter()
                .zip(&other.state.groups)
                .all(|(a, b)| a.id == b.id && a.members == b.members)
    }
    pub fn number(&mut self, window: WindowId) {
        if !self.state.numbers.iter().any(|(id, _)| *id == window) {
            self.next_number += 1;
            self.state.numbers.push((window, self.next_number));
        }
    }

    pub fn members(&self, target: WindowTarget) -> Result<Vec<WindowId>, String> {
        match target {
            WindowTarget::Window(id) => Ok(vec![id]),
            WindowTarget::Group(id) => self
                .state
                .group(id)
                .map(|g| g.members.clone())
                .ok_or_else(|| format!("Tab group ~{} is no longer available", id.0)),
        }
    }

    pub fn current_group(&self) -> Option<TabGroupId> {
        match self.state.target {
            Some(WindowTarget::Group(id)) => self.state.group(id).map(|g| g.id),
            Some(WindowTarget::Window(id)) => self.state.containing(id).map(|g| g.id),
            None => self
                .state
                .active
                .and_then(|id| self.state.containing(id).map(|g| g.id)),
        }
    }

    pub fn activate(&mut self, id: WindowId) {
        self.state.active = Some(id);
        if let Some(group) = self
            .state
            .groups
            .iter_mut()
            .find(|g| g.members.contains(&id))
        {
            group.active = id;
        }
    }

    /// A window means one member; a group means all its members. Never expand
    /// a source window into its old group implicitly.
    pub fn choose(&mut self, selected: WindowTarget) -> Result<(), String> {
        let incoming = self.members(selected)?;
        let active = match selected {
            WindowTarget::Window(id) => id,
            WindowTarget::Group(id) => self.state.group(id).ok_or("Tab group expired")?.active,
        };
        let Some(target) = self.state.target else {
            self.state.target = Some(match selected {
                WindowTarget::Window(id) => self
                    .state
                    .containing(id)
                    .map_or(selected, |g| WindowTarget::Group(g.id)),
                group => group,
            });
            self.activate(active);
            return Ok(());
        };
        let target_members = self.members(target)?;
        if target_members.len()
            + incoming
                .iter()
                .filter(|id| !target_members.contains(id))
                .count()
            > 256
        {
            return Err("A tab group can contain at most 256 windows".into());
        }
        if incoming.iter().all(|id| target_members.contains(id)) {
            self.activate(active);
            return Ok(());
        }
        let group_id = match target {
            WindowTarget::Group(id) => id,
            WindowTarget::Window(id) => {
                // Group labels describe live groups, not the number of groups
                // created since launch. Reuse gaps without renumbering survivors.
                let group_id = (1..=u32::MAX)
                    .map(TabGroupId)
                    .find(|id| self.state.group(*id).is_none())
                    .ok_or("No tab group number is available")?;
                self.state.groups.push(TabGroup {
                    id: group_id,
                    members: vec![id],
                    active: id,
                });
                group_id
            }
        };
        for id in &incoming {
            for group in &mut self.state.groups {
                if group.id != group_id {
                    Self::remove_member(group, *id);
                }
            }
        }
        let group = self
            .state
            .groups
            .iter_mut()
            .find(|g| g.id == group_id)
            .ok_or("Tab group expired")?;
        for id in incoming {
            if !group.members.contains(&id) {
                group.members.push(id);
            }
        }
        group.active = active;
        self.state.target = Some(WindowTarget::Group(group_id));
        self.state.active = Some(active);
        self.prune();
        Ok(())
    }

    pub fn drop_tabs(&mut self, drop: TabDrop) -> Result<(), String> {
        let incoming = self.members(drop.source)?;
        let destination = self.state.group(drop.target).ok_or("Drop target expired")?;
        if drop
            .before
            .is_some_and(|id| !destination.members.contains(&id))
        {
            return Err("Drop position expired".into());
        }
        if drop.source == WindowTarget::Group(drop.target) {
            return Ok(());
        }
        let before = drop.before.filter(|id| !incoming.contains(id));
        // Dropping onto the dragged tab itself is an unchanged position.
        if drop.before.is_some() && before.is_none() {
            return Ok(());
        }
        self.state.target = Some(WindowTarget::Group(drop.target));
        self.choose(drop.source)?;
        let group = self
            .state
            .groups
            .iter_mut()
            .find(|g| g.id == drop.target)
            .ok_or("Drop target expired")?;
        group.members.retain(|id| !incoming.contains(id));
        let index = before
            .and_then(|id| group.members.iter().position(|member| *member == id))
            .unwrap_or(group.members.len());
        group.members.splice(index..index, incoming);
        self.state.target = None;
        Ok(())
    }

    fn remove_member(group: &mut TabGroup, id: WindowId) {
        if let Some(index) = group.members.iter().position(|member| *member == id) {
            group.members.remove(index);
            if group.active == id && !group.members.is_empty() {
                group.active = group.members[index.min(group.members.len() - 1)];
            }
        }
    }

    pub fn remove_active(&mut self) -> Result<(), String> {
        let group_id = self.current_group().ok_or("Select a tab group first")?;
        let group = self
            .state
            .groups
            .iter_mut()
            .find(|g| g.id == group_id)
            .ok_or("Tab group expired")?;
        let removed = group.active;
        Self::remove_member(group, removed);
        self.state.active = group.members.first().map(|_| group.active);
        self.state.target = Some(WindowTarget::Group(group_id));
        self.prune();
        Ok(())
    }

    pub fn dissolve(&mut self) -> Result<(), String> {
        let id = self.current_group().ok_or("Select a tab group first")?;
        self.state.groups.retain(|g| g.id != id);
        self.state.target = None;
        Ok(())
    }

    pub fn cycle(&mut self, backwards: bool, reorder: bool) -> Result<(), String> {
        let id = self.current_group().ok_or("Select a tab group first")?;
        let group = self
            .state
            .groups
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or("Tab group expired")?;
        let index = group
            .members
            .iter()
            .position(|id| *id == group.active)
            .ok_or("Tab expired")?;
        let next = if backwards {
            (index + group.members.len() - 1) % group.members.len()
        } else {
            (index + 1) % group.members.len()
        };
        if reorder {
            group.members.swap(index, next);
        } else {
            group.active = group.members[next];
        }
        self.state.active = Some(group.active);
        Ok(())
    }

    pub fn forget(&mut self, id: WindowId) {
        for group in &mut self.state.groups {
            Self::remove_member(group, id);
        }
        self.state.numbers.retain(|(window, _)| *window != id);
        if self.state.active == Some(id) {
            self.state.active = None;
        }
        if self.state.target == Some(WindowTarget::Window(id)) {
            self.state.target = None;
        }
        self.prune();
    }

    pub fn detach(&mut self, id: WindowId) {
        for group in &mut self.state.groups {
            Self::remove_member(group, id);
        }
        self.prune();
    }

    fn prune(&mut self) {
        self.state.groups.retain(|g| g.members.len() >= 2);
        if let Some(WindowTarget::Group(id)) = self.state.target
            && self.state.group(id).is_none()
        {
            self.state.target = None;
        }
    }

    pub fn restore(&mut self, mut before: Self) {
        // Window numbers remain stable across history; group labels are restored
        // with the complete membership checkpoint.
        before.next_number = self.next_number;
        before.state.numbers.clone_from(&self.state.numbers);
        *self = before;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window(id: u64) -> WindowTarget {
        WindowTarget::Window(WindowId(id))
    }
    #[test]
    fn new_groups_reuse_free_labels_without_renumbering_live_groups() {
        let mut groups = Groups::default();
        groups.choose(window(1)).unwrap();
        groups.choose(window(2)).unwrap();
        groups.state.target = None;
        groups.choose(window(3)).unwrap();
        groups.choose(window(4)).unwrap();
        let before = groups.clone();
        groups.state.target = Some(WindowTarget::Group(TabGroupId(1)));
        groups.dissolve().unwrap();
        groups.choose(window(5)).unwrap();
        groups.choose(window(6)).unwrap();
        assert_eq!(
            groups.state.containing(WindowId(5)).unwrap().id,
            TabGroupId(1)
        );
        assert_eq!(
            groups.state.containing(WindowId(3)).unwrap().id,
            TabGroupId(2)
        );
        groups.restore(before);
        assert_eq!(
            groups.state.group(TabGroupId(1)).unwrap().members,
            [WindowId(1), WindowId(2)]
        );
        groups.state.target = Some(WindowTarget::Group(TabGroupId(1)));
        groups.dissolve().unwrap();
        groups.state.target = Some(WindowTarget::Group(TabGroupId(2)));
        groups.dissolve().unwrap();
        for _ in 0..3 {
            groups.choose(window(1)).unwrap();
            groups.choose(window(2)).unwrap();
            assert_eq!(groups.state.groups[0].id, TabGroupId(1));
            groups.dissolve().unwrap();
        }
    }
    #[test]
    fn consecutive_groups_and_explicit_whole_group_merge() {
        let mut groups = Groups::default();
        for id in 1..=4 {
            groups.number(WindowId(id));
        }
        groups.choose(window(1)).unwrap();
        groups.choose(window(2)).unwrap();
        groups.state.target = None;
        groups.choose(window(3)).unwrap();
        groups.choose(window(4)).unwrap();
        assert_eq!(groups.state.groups.len(), 2);
        groups.state.target = None;
        groups.choose(WindowTarget::Group(TabGroupId(1))).unwrap();
        groups.choose(WindowTarget::Group(TabGroupId(2))).unwrap();
        assert_eq!(groups.state.groups.len(), 1);
        assert_eq!(groups.state.groups[0].members, [1, 2, 3, 4].map(WindowId));
        assert_eq!(groups.state.numbers[2], (WindowId(3), 3));
    }
    #[test]
    fn window_selection_transfers_only_one_member() {
        let mut groups = Groups::default();
        for id in [1, 2, 3] {
            groups.choose(window(id)).unwrap();
        }
        groups.state.target = None;
        groups.choose(window(4)).unwrap();
        groups.choose(window(2)).unwrap();
        assert_eq!(
            groups.state.group(TabGroupId(1)).unwrap().members,
            [WindowId(1), WindowId(3)]
        );
        assert_eq!(
            groups.state.group(TabGroupId(2)).unwrap().members,
            [WindowId(4), WindowId(2)]
        );
        groups.remove_active().unwrap();
        assert_eq!(groups.state.groups.len(), 1);
        assert_eq!(groups.state.target, None);
    }
    #[test]
    fn selecting_an_existing_member_only_activates_it() {
        let mut groups = Groups::default();
        groups.choose(window(1)).unwrap();
        groups.choose(window(2)).unwrap();
        groups.choose(window(1)).unwrap();
        assert_eq!(groups.state.groups[0].members, [WindowId(1), WindowId(2)]);
        assert_eq!(groups.state.groups[0].active, WindowId(1));
        groups.forget(WindowId(1));
        assert!(groups.state.groups.is_empty());
    }
}
