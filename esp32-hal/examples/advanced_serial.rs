//! This shows how to configure UART
//! You can short the TX and RX pin and see it reads what was written.
//! Additionally you can connect a logic analzyer to TX and see how the changes
//! of the configuration change the output signal.

#![no_std]
#![no_main]

use esp32_hal::{
    clock::ClockControl,
    gpio::IO,
    pac::Peripherals,
    prelude::*,
    serial::{
        config::{Config, DataBits, Parity, StopBits},
        TxRxPins,
    },
    timer::TimerGroup,
    Delay,
    Rtc,
    Serial,
};
use esp_backtrace as _;
use esp_println::println;
use nb::block;
use xtensa_lx_rt::entry;

#[entry]
fn main() -> ! {
    let peripherals = Peripherals::take().unwrap();
    let system = peripherals.DPORT.split();
    let clocks = ClockControl::boot_defaults(system.clock_control).freeze();

    let timer_group0 = TimerGroup::new(peripherals.TIMG0, &clocks);
    let mut wdt = timer_group0.wdt;
    let mut rtc = Rtc::new(peripherals.RTC_CNTL);

    // Disable MWDT and RWDT (Watchdog) flash boot protection
    wdt.disable();
    rtc.rwdt.disable();

    let config = Config {
        baudrate: 115200,
        data_bits: DataBits::DataBits8,
        parity: Parity::ParityNone,
        stop_bits: StopBits::STOP1,
    };

    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let pins = TxRxPins::new_tx_rx(
        io.pins.gpio16.into_push_pull_output(),
        io.pins.gpio17.into_floating_input(),
    );

    let mut serial1 = Serial::new_with_config(peripherals.UART1, Some(config), Some(pins), &clocks);

    let mut delay = Delay::new(&clocks);

    println!("Start");
    loop {
        serial1.write(0x42).ok();
        let read = block!(serial1.read());

        match read {
            Ok(read) => println!("Read {:02x}", read),
            Err(err) => println!("Error {:?}", err),
        }

        delay.delay_ms(250u32);
    }
}
