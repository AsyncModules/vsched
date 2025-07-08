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
    std::fs::write(&out_path, mut_cfg).unwrap();

    println!("cargo:rerun-if-changed=src/*");
    println!("cargo:rerun-if-changed={}", out_path.display());
}
