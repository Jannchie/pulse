//! cargo run --release -p pulse-probe --example startup
fn main() {
    for i in pulse_probe::startup::list() {
        println!(
            "{:<5} {:<28} {:<26} {:<20} {}",
            if i.enabled { "启用" } else { "禁用" },
            i.name,
            i.source.label(),
            i.publisher.as_deref().unwrap_or("-"),
            i.path.as_deref().unwrap_or(&i.command)
        );
    }
}
