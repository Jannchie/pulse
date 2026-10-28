//! cargo run --release -p pulse-probe --example devices
fn main() {
    let mut c = pulse_probe::Collector::new();
    c.sample();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let s = c.sample();
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let i = &s.cpu_info;
    println!(
        "CPU {} | 基准 {} MHz 当前 {:?} MHz 性能核 {:?} 能效核 {:?} | {} 插槽 {} 内核 {} 逻辑 | L1 {} KB L2 {} MB L3 {} MB | 开机 {:?}",
        i.name,
        i.base_mhz,
        s.cpu_mhz,
        s.cpu_mhz_performance,
        s.cpu_mhz_efficient,
        i.sockets,
        i.cores,
        i.logical,
        i.l1 >> 10,
        i.l2 >> 20,
        i.l3 >> 20,
        s.uptime
    );
    let m = &s.memory;
    println!(
        "内存 总 {:.1} 使用中 {:.1} 可用 {:.1} | 提交 {:.1}/{:.1} 峰值 {:.1} | 缓存 {:.1} 分页池 {:.2} 非分页 {:.2} | {} 进程 {} 线程 {} 句柄",
        gb(m.total),
        gb(m.in_use()),
        gb(m.available),
        gb(m.commit_used),
        gb(m.commit_limit),
        gb(m.commit_peak),
        gb(m.cache),
        gb(m.paged_pool),
        gb(m.nonpaged_pool),
        m.processes,
        m.threads,
        m.handles
    );
    if let Some(l) = &m.lists {
        println!(
            "  列表 空闲 {:.1} 备用 {:.1} 已修改 {:.2}",
            gb(l.free),
            gb(l.standby),
            gb(l.modified)
        );
    }
    for mm in s.memory_modules.iter() {
        println!(
            "  内存条 {} {} {} {:.0}GB {} {} MT/s {}",
            mm.locator,
            mm.manufacturer,
            mm.part_number,
            gb(mm.size),
            mm.kind,
            mm.speed,
            mm.form_factor()
        );
    }
    for n in &s.nics {
        println!(
            "网卡 #{} {} ({}) {} {} {} Mbps v4 {:?} v6 {} 个 ↓{} ↑{}",
            n.index,
            n.name,
            n.description,
            n.kind,
            if n.connected {
                "已连接"
            } else {
                "未连接"
            },
            n.link_speed / 1_000_000,
            n.ipv4,
            n.ipv6.len(),
            n.rx_rate,
            n.tx_rate
        );
    }
    println!("采集 {:?}", s.collect_time);
}
