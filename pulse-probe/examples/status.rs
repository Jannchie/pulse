//! cargo run --release -p pulse-probe --example status
fn main() {
    let mut c = pulse_probe::Collector::new();
    let s = c.sample();
    let apps: Vec<_> = s.processes.iter().filter(|p| p.window.is_some()).collect();
    println!(
        "应用 {} 个，后台 {} 个",
        apps.len(),
        s.processes.len() - apps.len()
    );
    for p in apps.iter().take(12) {
        let w = p.window.as_ref().unwrap();
        println!(
            "  {:<24} {:<40} hung={} prio={:?} eff={}",
            p.name,
            w.title.chars().take(38).collect::<String>(),
            w.hung,
            p.priority,
            p.efficiency
        );
    }
    for p in s.processes.iter().filter(|p| p.suspended).take(8) {
        println!("已挂起 {} ({})", p.name, p.pid);
    }
    for p in s.processes.iter().filter(|p| p.efficiency).take(8) {
        println!("效率模式 {} ({})", p.name, p.pid);
    }
    println!("采集 {:?}", s.collect_time);
}
