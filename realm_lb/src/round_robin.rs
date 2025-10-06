use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Balance, Token, HealthCheckConfig};

/// Round-robin node.
#[derive(Debug)]
struct Node {
    cw: i16,
    ew: u8,
    weight: u8,
    token: Token,
    
    fails: AtomicU32,
    checked: AtomicU32,
}

/// Round robin balancer.
#[derive(Debug)]
pub struct RoundRobin {
    nodes: Mutex<Vec<Node>>,
    total: u8,
    config: Option<HealthCheckConfig>,
}

fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32
}

impl Balance for RoundRobin {
    type State = ();

    fn total(&self) -> u8 {
        self.total
    }

    fn new(weights: &[u8], config: Option<HealthCheckConfig>) -> Self {
        assert!(weights.len() <= u8::MAX as usize);

        if weights.len() <= 1 {
            return Self {
                nodes: Mutex::new(Vec::new()),
                total: weights.len() as u8,
                config,
            };
        }

        let nodes = weights
            .iter()
            .enumerate()
            .map(|(i, w)| Node {
                ew: *w,
                cw: 0,
                weight: *w,
                token: Token(i as u8),
                fails: AtomicU32::new(0),
                checked: AtomicU32::new(0),
            })
            .collect();
        Self {
            nodes: Mutex::new(nodes),
            total: weights.len() as u8,
            config,
        }
    }

    #[allow(clippy::significant_drop_in_scrutinee)]
    fn next(&self, _: &Self::State) -> Option<Token> {
        if self.total <= 1 {
            return Some(Token(0));
        }

        let now = now_secs();
        let mut nodes = self.nodes.lock().unwrap();
        let mut tw: i16 = 0;
        let mut best: Option<&mut Node> = None;
        
        let mut first_token = None;
        
        for p in nodes.iter_mut() {
            if let Some(cfg) = &self.config {
                let fails = p.fails.load(Ordering::Relaxed);
                let checked = p.checked.load(Ordering::Relaxed);
                
                if first_token.is_none() {
                    first_token = Some(p.token);
                }
                
                if fails >= cfg.max_fails {
                    if now < checked {
                        continue;
                    }
                    p.checked.store(0, Ordering::Relaxed);
                }
            }
            
            tw += p.ew as i16;
            p.cw += p.ew as i16;

            if let Some(ref x) = best {
                if p.cw > x.cw {
                    best = Some(p);
                }
            } else {
                best = Some(p);
            }
        }
        
        if best.is_none() && self.config.is_some() {
            return first_token;
        }

        best.map(|x| {
            x.cw -= tw;
            
            // Gradual ew recovery (only when selected)
            if x.ew < x.weight {
                x.ew += 1;
            }
            
            x.token
        })
    }
    
    fn on_success(&self, token: Token) {
        if self.config.is_none() {
            return;
        }
        
        let nodes = self.nodes.lock().unwrap();
        
        if let Some(node) = nodes.iter().find(|n| n.token == token) {
            node.fails.store(0, Ordering::Relaxed);
        }
    }
    
    fn on_failure(&self, token: Token) {
        if self.config.is_none() {
            return;
        }
        
        let cfg = self.config.as_ref().unwrap();
        let mut nodes = self.nodes.lock().unwrap();
        
        if let Some(node) = nodes.iter_mut().find(|n| n.token == token) {
            let fails = node.fails.fetch_add(1, Ordering::Relaxed) + 1;
            
            if fails >= cfg.max_fails {
                let now = now_secs();
                node.checked.store(now + cfg.fail_timeout_secs, Ordering::Relaxed);
                node.ew = 1;  // Start gradual recovery from minimum weight
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use average::{Max, Mean, Min};

    #[test]
    fn rr_same_weight() {
        let rr = RoundRobin::new(&vec![1; 255], None);
        let mut distro = [0f64; 255];

        for _ in 0..1_000_000 {
            let token = rr.next(&()).unwrap();
            distro[token.0 as usize] += 1 as f64;
        }

        let diffs: Vec<f64> = distro
            .iter()
            .map(|x| *x / 1_000_000.0 - 1.0 / 255.0)
            .map(f64::abs)
            .inspect(|x| assert!(x < &1e-3))
            .collect();

        let min_diff: Min = diffs.iter().collect();
        let max_diff: Max = diffs.iter().collect();
        let mean_diff: Mean = diffs.iter().collect();

        println!("{:?}", distro);
        println!("min diff: {}", min_diff.min());
        println!("max diff: {}", max_diff.max());
        println!("mean diff: {}", mean_diff.mean());
    }

    #[test]
    fn rr_all_weights() {
        let weights: Vec<u8> = (1..=255).collect();
        let total_weight: f64 = weights.iter().map(|x| *x as f64).sum();
        let rr = RoundRobin::new(&weights, None);
        let mut distro = [0f64; 255];

        for _ in 0..1_000_000 {
            let token = rr.next(&()).unwrap();
            distro[token.0 as usize] += 1 as f64;
        }

        let diffs: Vec<f64> = distro
            .iter()
            .enumerate()
            .map(|(i, x)| *x / 1_000_000.0 - (i as f64 + 1.0) / total_weight)
            .map(f64::abs)
            .inspect(|x| assert!(x < &1e-3))
            .collect();

        let min_diff: Min = diffs.iter().collect();
        let max_diff: Max = diffs.iter().collect();
        let mean_diff: Mean = diffs.iter().collect();

        println!("{:?}", distro);
        println!("min diff: {}", min_diff.min());
        println!("max diff: {}", max_diff.max());
        println!("mean diff: {}", mean_diff.mean());
    }
}
