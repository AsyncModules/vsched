use std::fs;
use std::io::Write;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("mut_cfgs.rs");
    let rq_cap: usize = option_env!("RQ_CAP").unwrap_or("1").parse().unwrap();
    let smp: usize = option_env!("SMP").unwrap_or("1").parse().unwrap();
    assert!(rq_cap.is_power_of_two());

    let mut_cfg = format!(
        r#"
pub const RQ_CAP: usize = {};
pub const SMP: usize = {};
"#,
        rq_cap, smp
    );
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(out_path)
        .unwrap();
    f.write_all(mut_cfg.as_bytes()).unwrap();

    println!("cargo:rerun-if-changed=src/*");
}
