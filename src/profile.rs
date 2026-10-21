use crate::cli::{Sort, ViewOptions};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Frame {
    pub symbol: String,
    pub image: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Stack {
    pub process: String,
    pub thread: String,
    /// xctrace orders frames from the sampled leaf towards the root.
    pub frames: Vec<Frame>,
}

#[derive(Debug, Default)]
pub struct Profile {
    pub source: String,
    pub schema: String,
    pub run: Option<u32>,
    pub unit: String,
    pub stacks: BTreeMap<Stack, f64>,
    pub samples: u64,
    pub missing_stacks: u64,
    pub total: f64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Key {
    pub process: String,
    pub frame: Frame,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: Key,
    pub overhead: f64,
    pub self_percent: f64,
    pub baseline: f64,
    pub delta: f64,
}

#[derive(Default)]
struct Weights {
    inclusive: f64,
    own: f64,
}

pub fn percentage(weight: f64, total: f64) -> f64 {
    if total > 0.0 {
        weight / total * 100.0
    } else {
        0.0
    }
}

impl Profile {
    fn weights(&self) -> BTreeMap<Key, Weights> {
        let mut result: BTreeMap<Key, Weights> = BTreeMap::new();
        for (stack, weight) in &self.stacks {
            let mut seen = BTreeSet::new();
            for (index, frame) in stack.frames.iter().enumerate() {
                if seen.insert(frame) {
                    let entry = result
                        .entry(Key {
                            process: stack.process.clone(),
                            frame: frame.clone(),
                        })
                        .or_default();
                    entry.inclusive += weight;
                    if index == 0 {
                        entry.own += weight;
                    }
                }
            }
        }
        result
    }

    pub fn metadata(&self) -> String {
        format!(
            "{}\nSchema: {} | run: {} | {} samples | total: {:.0} {} | missing stacks: {}",
            self.source,
            self.schema,
            self.run
                .map_or_else(|| "exported".into(), |r| r.to_string()),
            self.samples,
            self.total,
            self.unit,
            self.missing_stacks
        )
    }

    pub fn callchains(&self, key: &Key, children: bool) -> Vec<(String, f64)> {
        let mut chains: BTreeMap<String, f64> = BTreeMap::new();
        for (stack, weight) in &self.stacks {
            if stack.process != key.process {
                continue;
            }
            let Some(position) = stack.frames.iter().position(|f| f == &key.frame) else {
                continue;
            };
            if !children && position != 0 {
                continue;
            }
            let chain = stack
                .frames
                .iter()
                .map(|f| format!("{} [{}]", f.symbol, f.image))
                .collect::<Vec<_>>()
                .join(" <- ");
            *chains
                .entry(format!("{}: {chain}", stack.thread))
                .or_default() += weight;
        }
        let mut chains: Vec<_> = chains
            .into_iter()
            .map(|(chain, weight)| (chain, percentage(weight, self.total)))
            .collect();
        chains.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        chains
    }
}

pub fn entries(profile: &Profile, baseline: Option<&Profile>, children: bool) -> Vec<Entry> {
    let weights = profile.weights();
    let before = baseline.map(Profile::weights).unwrap_or_default();
    let keys: BTreeSet<_> = weights.keys().chain(before.keys()).cloned().collect();
    keys.into_iter()
        .map(|key| {
            let value = |map: &BTreeMap<Key, Weights>| {
                map.get(&key)
                    .map_or(0.0, |w| if children { w.inclusive } else { w.own })
            };
            let overhead = percentage(value(&weights), profile.total);
            let baseline = percentage(value(&before), baseline.map_or(0.0, |p| p.total));
            let self_percent = percentage(weights.get(&key).map_or(0.0, |w| w.own), profile.total);
            Entry {
                key,
                overhead,
                self_percent,
                baseline,
                delta: overhead - baseline,
            }
        })
        .collect()
}

pub fn matches(value: &str, filter: Option<&str>) -> bool {
    filter.is_none_or(|f| value.to_lowercase().contains(&f.to_lowercase()))
}

pub fn filter_entries(entries: &mut Vec<Entry>, options: &ViewOptions) {
    entries.retain(|entry| {
        matches(&entry.key.frame.symbol, options.symbol.as_deref())
            && matches(&entry.key.process, options.comm.as_deref())
            && matches(&entry.key.frame.image, options.dso.as_deref())
            && entry.overhead.max(entry.baseline) >= options.percent_limit
            && entry.overhead.max(entry.baseline) > 0.0
    });
}

pub fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        match sort {
            Sort::Overhead => b.overhead.total_cmp(&a.overhead),
            Sort::SelfWeight => b.self_percent.total_cmp(&a.self_percent),
            Sort::Symbol => a.key.frame.symbol.cmp(&b.key.frame.symbol),
            Sort::Dso => a.key.frame.image.cmp(&b.key.frame.image),
            Sort::Comm => a.key.process.cmp(&b.key.process),
            Sort::Delta => b.delta.abs().total_cmp(&a.delta.abs()),
        }
        .then_with(|| a.key.cmp(&b.key))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(symbols: &[&str], weight: f64) -> Profile {
        let stack = Stack {
            process: "app".into(),
            thread: "main".into(),
            frames: symbols
                .iter()
                .map(|s| Frame {
                    symbol: (*s).into(),
                    image: "app".into(),
                })
                .collect(),
        };
        Profile {
            total: weight,
            stacks: [(stack, weight)].into(),
            ..Profile::default()
        }
    }
    #[test]
    fn inclusive_deduplicates_recursion_and_self_is_leaf_only() {
        let p = profile(&["leaf", "recursive", "recursive", "root"], 10.0);
        let e = entries(&p, None, true);
        assert_eq!(e.len(), 3);
        assert!(e.iter().all(|e| (e.overhead - 100.0).abs() < f64::EPSILON));
        assert!((e.iter().map(|e| e.self_percent).sum::<f64>() - 100.0).abs() < f64::EPSILON);
    }
    #[test]
    fn diff_includes_added_and_removed_symbols_normalized_independently() {
        let before = profile(&["old"], 100.0);
        let after = profile(&["new"], 200.0);
        let e = entries(&after, Some(&before), false);
        assert_eq!(e.len(), 2);
        assert!(
            e.iter()
                .any(|e| e.key.frame.symbol == "old" && (e.delta + 100.0).abs() < f64::EPSILON)
        );
        assert!(
            e.iter()
                .any(|e| e.key.frame.symbol == "new" && (e.delta - 100.0).abs() < f64::EPSILON)
        );
    }
}
