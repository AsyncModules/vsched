use memmap::MmapOptions;
use std::fs;
use std::io::Write;

fn map_vsched() {
    // All peripherals can be described by an offset from the Peripheral Base Address, which starts at:
    // 0x20000000 on the Raspberry Pi model 1
    // 0x3F000000 on the models 2 and 3.
    const PERIPHERAL_BASE_ADDRESS: u64 = 0x3F000000;

    // The System Timer is a hardware clock that can be used to keep time and generate interrupts after a certain time.
    // It is located at offset 0x3000 from the peripheral base.
    const SYSTEM_TIMER_OFFSET: u64 = 0x3000;

    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/mem")
        .unwrap();

    // Create a new memory map builder.
    let mmap = unsafe {
        MmapOptions::new()
            .offset(PERIPHERAL_BASE_ADDRESS + SYSTEM_TIMER_OFFSET + 4)
            .len(4096)
            .map_mut(&f)
            .unwrap()
    };

    let bytes = mmap.get(0..4).unwrap();
    loop {
        println!("{:?}", bytes);
    }
}
