//! cargo run --release -p pulse-probe --example top
fn main() {
    let mut c = pulse_probe::Collector::new();
    let t = std::time::Instant::now();
    c.sample();
    println!("首次采集（含打开句柄、取路径、GPU 基线）{:?}", t.elapsed());
    // 期间制造一点网络流量，验证 ETW 逐进程统计。
    let mut dl = std::process::Command::new("curl")
        .args([
            "-s",
            "-o",
            "NUL",
            "--limit-rate",
            "5M",
            "https://speed.cloudflare.com/__down?bytes=50000000",
        ])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    c.sample();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let mut s = c.sample();
    let _ = dl.kill();
    let _ = dl.wait();
    println!(
        "{} 个进程，CPU {:.1}%，GPU {:.1}% {:?}，网络 ↓{} ↑{} B/s，net {:?}，elevated {}，采集 {:?}",
        s.processes.len(),
        s.cpu_total,
        s.gpu_total,
        s.gpus
            .iter()
            .map(|g| (&g.name, g.usage))
            .collect::<Vec<_>>(),
        s.net_rx,
        s.net_tx,
        s.net_status,
        s.elevated,
        s.collect_time,
    );
    let count =
        |f: &dyn Fn(&pulse_probe::Process) -> bool| s.processes.iter().filter(|p| f(p)).count();
    println!(
        "有路径 {}，有描述 {}，有命令行 {}，有监听端口 {}",
        count(&|p| p.path.is_some()),
        count(&|p| p.description.is_some()),
        count(&|p| p.command_line.is_some()),
        count(&|p| !p.ports.is_empty()),
    );
    for p in s
        .processes
        .iter()
        .filter(|p| {
            p.ports
                .iter()
                .any(|x| x.proto == pulse_probe::ports::Proto::Tcp)
        })
        .take(6)
    {
        println!(
            "  {:<22} {:<30} {:?}",
            p.name,
            p.description.as_deref().unwrap_or("-"),
            p.ports.iter().map(|x| x.port).collect::<Vec<_>>()
        );
    }
    if let Some(p) = s
        .processes
        .iter()
        .find(|p| p.name == "node.exe" && p.command_line.is_some())
    {
        println!("  node 命令行: {}", p.command_line.as_deref().unwrap());
    }
    s.processes
        .sort_by_key(|p| std::cmp::Reverse(p.net_rx + p.net_tx));
    println!("-- 网络 top");
    for p in s.processes.iter().take(5) {
        println!(
            "{:>7} {:<28} ↓{:>10} ↑{:>10}",
            p.pid, p.name, p.net_rx, p.net_tx
        );
    }
    s.processes.sort_by(|a, b| b.gpu.total_cmp(&a.gpu));
    println!("-- GPU top");
    for p in s.processes.iter().take(5) {
        println!("{:>7} {:<28} {:>5.1}%", p.pid, p.name, p.gpu);
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let n = 50;
    let t = std::time::Instant::now();
    for _ in 0..n {
        c.sample();
    }
    println!("稳态平均单次采集 {:?}", t.elapsed() / n);
}
