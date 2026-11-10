//! 席位计数：起跑预约和正在跑的会话按任务合并计数，照 DSH `WorkerSlots`。
//!
//! 这里的键是任务 id；在跑集合由调用方从事实算出来传进来，计数原语自己不读时钟。

use std::collections::{BTreeMap, BTreeSet};

/// 预约席位的起跑方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    /// 新任务的首次起跑。
    Launch,
    /// 停了、失败了的任务被唤醒。
    Wake,
    /// 用户在任务对话里说话，叫醒一个没在跑的任务；可以多用 1 个席位。
    Person,
}

/// 没有空席位；调用方按 `Later` 处理，不计失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Later;

/// 还没释放的起跑预约。
#[derive(Debug, Default)]
pub struct WorkerSlots {
    starting: BTreeMap<String, SlotKind>,
}

impl WorkerSlots {
    pub fn new() -> Self {
        Self::default()
    }

    /// 在跑任务和起跑预约合并后的席位数。
    pub fn used(&self, running: &[String]) -> usize {
        let mut keys: BTreeSet<&str> = running.iter().map(String::as_str).collect();
        keys.extend(self.starting.keys().map(String::as_str));
        keys.len()
    }

    /// 还能同时起跑几个 launch：席位数减去在跑的和唤醒预约。
    ///
    /// 已经在飞的 launch 预约不扣：它自己就是一个正在起跑的 launch；席位是否满由
    /// `reserve` 再看一次 `used` 决定。
    pub fn launches(&self, running: &[String], max: usize) -> usize {
        let mut keys: BTreeSet<&str> = running.iter().map(String::as_str).collect();
        keys.extend(
            self.starting
                .iter()
                .filter(|(_, kind)| **kind != SlotKind::Launch)
                .map(|(key, _)| key.as_str()),
        );
        max.saturating_sub(keys.len())
    }

    /// 预约一个起跑席位；同一个键已经预约过就不重复占。
    pub fn reserve(
        &mut self,
        key: &str,
        kind: SlotKind,
        running: &[String],
        max: usize,
    ) -> Result<(), Later> {
        // 同一个键重试时保留原来的种类和位置：它已经占着一个起跑席位。
        if self.starting.contains_key(key) {
            return Ok(());
        }
        // person 是唯一能多用 1 个席位的起跑；总量只多这 1 个，不按任务各加。
        let extra = usize::from(kind == SlotKind::Person);
        if self.used(running) >= max.saturating_add(extra) {
            return Err(Later);
        }
        self.starting.insert(key.to_owned(), kind);
        Ok(())
    }

    /// 释放一个起跑预约；没预约过的键什么也不做。
    pub fn free(&mut self, key: &str) {
        self.starting.remove(key);
    }
}

#[cfg(test)]
#[path = "slots_tests.rs"]
mod tests;
