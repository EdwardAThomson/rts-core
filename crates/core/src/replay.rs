//! Commands queued for a tick, and the log of every command with its tick. A replay is the starting data, the
//! seed and this log; re-running them reproduces the game exactly.

use std::collections::BTreeMap;

/// A command as stored in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logged<C> {
    pub tick: u32,
    pub command: C,
}

/// Commands waiting for the tick they apply on, plus the log of everything ever queued. A `BTreeMap` keeps the
/// ticks in order; commands for one tick keep the order they were given in.
#[derive(Clone, Debug)]
pub struct CommandQueue<C> {
    queued: BTreeMap<u32, Vec<C>>,
    log: Vec<Logged<C>>,
}

impl<C> Default for CommandQueue<C> {
    fn default() -> Self {
        Self { queued: BTreeMap::new(), log: Vec::new() }
    }
}

impl<C: Clone> CommandQueue<C> {
    /// Queue a command to apply at the start of `tick`, and log it.
    pub fn push(&mut self, tick: u32, command: C) {
        self.queued.entry(tick).or_default().push(command.clone());
        self.log.push(Logged { tick, command });
    }

    /// Remove and return the commands for `tick`, in the order they were queued.
    pub fn take(&mut self, tick: u32) -> Vec<C> {
        self.queued.remove(&tick).unwrap_or_default()
    }

    pub fn log(&self) -> &[Logged<C>] {
        &self.log
    }
}
