//! General-purpose timers

use core::marker::PhantomData;

use embedded_hal::{
    timer::{Cancel, CountDown, Periodic},
    watchdog::{Watchdog, WatchdogDisable, WatchdogEnable},
};
use fugit::{HertzU32, MicrosDurationU64};
use void::Void;

use crate::{
    clock::Clocks,
    pac::{timg0::RegisterBlock, TIMG0, TIMG1},
};

/// Custom timer error type
#[derive(Debug)]
pub enum Error {
    TimerActive,
    TimerInactive,
    AlarmInactive,
}

// A timergroup consisting of up to 2 timers (chip dependent) and a watchdog
// timer
pub struct TimerGroup<T>
where
    T: TimerGroupInstance,
{
    pub timer0: Timer<Timer0<T>>,
    #[cfg(not(feature = "esp32c3"))]
    pub timer1: Timer<Timer1<T>>,
    pub wdt: Wdt<T>,
}

pub trait TimerGroupInstance {
    fn register_block() -> *const RegisterBlock;
}

impl TimerGroupInstance for TIMG0 {
    #[inline(always)]
    fn register_block() -> *const RegisterBlock {
        crate::pac::TIMG0::PTR
    }
}

impl TimerGroupInstance for TIMG1 {
    #[inline(always)]
    fn register_block() -> *const RegisterBlock {
        crate::pac::TIMG1::PTR
    }
}

impl<T> TimerGroup<T>
where
    T: TimerGroupInstance,
{
    pub fn new(_timer_group: T, clocks: &Clocks) -> Self {
        let timer0 = Timer::new(
            Timer0 {
                phantom: PhantomData::default(),
            },
            clocks.apb_clock,
        );

        #[cfg(not(feature = "esp32c3"))]
        let timer1 = Timer::new(
            Timer1 {
                phantom: PhantomData::default(),
            },
            clocks.apb_clock,
        );

        let wdt = Wdt::new();

        Self {
            timer0,
            #[cfg(not(feature = "esp32c3"))]
            timer1,
            wdt,
        }
    }
}

/// General-purpose timer
pub struct Timer<T> {
    timg: T,
    apb_clk_freq: HertzU32,
}

/// Timer driver
impl<T> Timer<T>
where
    T: Instance,
{
    /// Create a new timer instance
    pub fn new(timg: T, apb_clk_freq: HertzU32) -> Self {
        // TODO: this currently assumes APB_CLK is being used, as we don't yet have a
        //       way to select the XTAL_CLK.
        Self { timg, apb_clk_freq }
    }

    /// Return the raw interface to the underlying timer instance
    pub fn free(self) -> T {
        self.timg
    }

    /// Listen for interrupt
    pub fn listen(&mut self) {
        self.timg.listen();
    }

    /// Stop listening for interrupt
    pub fn unlisten(&mut self) {
        self.timg.unlisten();
    }

    /// Clear interrupt status
    pub fn clear_interrupt(&mut self) {
        self.timg.clear_interrupt();
    }

    /// Check if the interrupt is asserted
    pub fn is_interrupt_set(&self) -> bool {
        self.timg.is_interrupt_set()
    }

    /// Read current raw timer value in timer ticks
    pub fn read_raw(&self) -> u64 {
        self.timg.read_raw()
    }
}

/// Timer peripheral instance
pub trait Instance {
    fn reset_counter(&mut self);

    fn set_counter_active(&mut self, state: bool);

    fn is_counter_active(&self) -> bool;

    fn set_counter_decrementing(&mut self, decrementing: bool);

    fn set_auto_reload(&mut self, auto_reload: bool);

    fn set_alarm_active(&mut self, state: bool);

    fn is_alarm_active(&self) -> bool;

    fn load_alarm_value(&mut self, value: u64);

    fn listen(&mut self);

    fn unlisten(&mut self);

    fn clear_interrupt(&mut self);

    fn read_raw(&self) -> u64;

    fn divider(&self) -> u32;

    fn is_interrupt_set(&self) -> bool;
}

pub struct Timer0<TG> {
    phantom: PhantomData<TG>,
}

/// Timer peripheral instance
impl<TG> Instance for Timer0<TG>
where
    TG: TimerGroupInstance,
{
    fn reset_counter(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t0loadlo.write(|w| unsafe { w.load_lo().bits(0) });

        reg_block.t0loadhi.write(|w| unsafe { w.load_hi().bits(0) });

        reg_block.t0load.write(|w| unsafe { w.load().bits(1) });
    }

    fn set_counter_active(&mut self, state: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t0config.modify(|_, w| w.en().bit(state));
    }

    fn is_counter_active(&self) -> bool {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t0config.read().en().bit_is_set()
    }

    fn set_counter_decrementing(&mut self, decrementing: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .t0config
            .modify(|_, w| w.increase().bit(!decrementing));
    }

    fn set_auto_reload(&mut self, auto_reload: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .t0config
            .modify(|_, w| w.autoreload().bit(auto_reload));
    }

    fn set_alarm_active(&mut self, state: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t0config.modify(|_, w| w.alarm_en().bit(state));
    }

    fn is_alarm_active(&self) -> bool {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t0config.read().alarm_en().bit_is_set()
    }

    fn load_alarm_value(&mut self, value: u64) {
        let value = value & 0x3F_FFFF_FFFF_FFFF;
        let high = (value >> 32) as u32;
        let low = (value & 0xFFFF_FFFF) as u32;

        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .t0alarmlo
            .write(|w| unsafe { w.alarm_lo().bits(low) });

        reg_block
            .t0alarmhi
            .write(|w| unsafe { w.alarm_hi().bits(high) });
    }

    fn listen(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        // always use level interrupt
        #[cfg(any(feature = "esp32", feature = "esp32s2"))]
        reg_block.t0config.modify(|_, w| w.level_int_en().set_bit());

        reg_block
            .int_ena_timers
            .modify(|_, w| w.t0_int_ena().set_bit());
    }

    fn unlisten(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .int_ena_timers
            .modify(|_, w| w.t0_int_ena().clear_bit());
    }

    fn clear_interrupt(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.int_clr_timers.write(|w| w.t0_int_clr().set_bit());
    }

    fn read_raw(&self) -> u64 {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t0update.write(|w| unsafe { w.bits(0) });

        let value_lo = reg_block.t0lo.read().bits() as u64;
        let value_hi = (reg_block.t0hi.read().bits() as u64) << 32;

        (value_lo | value_hi) as u64
    }

    fn divider(&self) -> u32 {
        let reg_block = unsafe { &*TG::register_block() };

        // From the ESP32 TRM, "11.2.1 16­-bit Prescaler and Clock Selection":
        //
        // "The prescaler can divide the APB clock by a factor from 2 to 65536.
        // Specifically, when TIMGn_Tx_DIVIDER is either 1 or 2, the clock divisor is 2;
        // when TIMGn_Tx_DIVIDER is 0, the clock divisor is 65536. Any other value will
        // cause the clock to be divided by exactly that value."
        match reg_block.t0config.read().divider().bits() {
            0 => 65536,
            1 | 2 => 2,
            n => n as u32,
        }
    }

    fn is_interrupt_set(&self) -> bool {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.int_raw_timers.read().t0_int_raw().bit_is_set()
    }
}

#[cfg(not(feature = "esp32c3"))]
pub struct Timer1<TG> {
    phantom: PhantomData<TG>,
}

/// Timer peripheral instance
#[cfg(not(feature = "esp32c3"))]
impl<TG> Instance for Timer1<TG>
where
    TG: TimerGroupInstance,
{
    fn reset_counter(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t1loadlo.write(|w| unsafe { w.load_lo().bits(0) });

        reg_block.t1loadhi.write(|w| unsafe { w.load_hi().bits(0) });

        reg_block.t1load.write(|w| unsafe { w.load().bits(1) });
    }

    fn set_counter_active(&mut self, state: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t1config.modify(|_, w| w.en().bit(state));
    }

    fn is_counter_active(&self) -> bool {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t1config.read().en().bit_is_set()
    }

    fn set_counter_decrementing(&mut self, decrementing: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .t1config
            .modify(|_, w| w.increase().bit(!decrementing));
    }

    fn set_auto_reload(&mut self, auto_reload: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .t1config
            .modify(|_, w| w.autoreload().bit(auto_reload));
    }

    fn set_alarm_active(&mut self, state: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t1config.modify(|_, w| w.alarm_en().bit(state));
    }

    fn is_alarm_active(&self) -> bool {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t1config.read().alarm_en().bit_is_set()
    }

    fn load_alarm_value(&mut self, value: u64) {
        let value = value & 0x3F_FFFF_FFFF_FFFF;
        let high = (value >> 32) as u32;
        let low = (value & 0xFFFF_FFFF) as u32;

        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .t1alarmlo
            .write(|w| unsafe { w.alarm_lo().bits(low) });

        reg_block
            .t1alarmhi
            .write(|w| unsafe { w.alarm_hi().bits(high) });
    }

    fn listen(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        // always use level interrupt
        #[cfg(any(feature = "esp32", feature = "esp32s2"))]
        reg_block.t1config.modify(|_, w| w.level_int_en().set_bit());

        reg_block
            .int_ena_timers
            .modify(|_, w| w.t1_int_ena().set_bit());
    }

    fn unlisten(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .int_ena_timers
            .modify(|_, w| w.t1_int_ena().clear_bit());
    }

    fn clear_interrupt(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.int_clr_timers.write(|w| w.t1_int_clr().set_bit());
    }

    fn read_raw(&self) -> u64 {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.t1update.write(|w| unsafe { w.bits(0) });

        let value_lo = reg_block.t1lo.read().bits() as u64;
        let value_hi = (reg_block.t1hi.read().bits() as u64) << 32;

        (value_lo | value_hi) as u64
    }

    fn divider(&self) -> u32 {
        let reg_block = unsafe { &*TG::register_block() };

        // From the ESP32 TRM, "11.2.1 16­-bit Prescaler and Clock Selection":
        //
        // "The prescaler can divide the APB clock by a factor from 2 to 65536.
        // Specifically, when TIMGn_Tx_DIVIDER is either 1 or 2, the clock divisor is 2;
        // when TIMGn_Tx_DIVIDER is 0, the clock divisor is 65536. Any other value will
        // cause the clock to be divided by exactly that value."
        match reg_block.t1config.read().divider().bits() {
            0 => 65536,
            1 | 2 => 2,
            n => n as u32,
        }
    }

    fn is_interrupt_set(&self) -> bool {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block.int_raw_timers.read().t1_int_raw().bit_is_set()
    }
}

fn timeout_to_ticks<T, F>(timeout: T, clock: F, divider: u32) -> u64
where
    T: Into<MicrosDurationU64>,
    F: Into<HertzU32>,
{
    let timeout: MicrosDurationU64 = timeout.into();
    let micros = timeout.to_micros();

    let clock: HertzU32 = clock.into();

    // TODO can we get this to not use doubles/floats
    let period = 1_000_000f64 / (clock.to_Hz() as f64 / divider as f64); // micros

    (micros as f64 / period) as u64
}

impl<T> CountDown for Timer<T>
where
    T: Instance,
{
    type Time = MicrosDurationU64;

    fn start<Time>(&mut self, timeout: Time)
    where
        Time: Into<Self::Time>,
    {
        self.timg.set_counter_active(false);
        self.timg.set_alarm_active(false);

        self.timg.reset_counter();

        // TODO: this currently assumes APB_CLK is being used, as we don't yet have a
        //       way to select the XTAL_CLK.
        // TODO: can we cache the divider (only get it on initialization)?
        let ticks = timeout_to_ticks(timeout, self.apb_clk_freq, self.timg.divider());
        self.timg.load_alarm_value(ticks);

        self.timg.set_counter_decrementing(false);
        self.timg.set_auto_reload(true);
        self.timg.set_counter_active(true);
        self.timg.set_alarm_active(true);
    }

    fn wait(&mut self) -> nb::Result<(), Void> {
        if !self.timg.is_counter_active() {
            panic!("Called wait on an inactive timer!")
        }

        if self.timg.is_interrupt_set() {
            self.timg.clear_interrupt();
            self.timg.set_alarm_active(true);

            Ok(())
        } else {
            Err(nb::Error::WouldBlock)
        }
    }
}

impl<T> Cancel for Timer<T>
where
    T: Instance,
{
    type Error = Error;

    fn cancel(&mut self) -> Result<(), Error> {
        if !self.timg.is_counter_active() {
            return Err(Error::TimerInactive);
        } else if !self.timg.is_alarm_active() {
            return Err(Error::AlarmInactive);
        }

        self.timg.set_counter_active(false);

        Ok(())
    }
}

impl<T> Periodic for Timer<T> where T: Instance {}

/// Watchdog timer
pub struct Wdt<TG> {
    phantom: PhantomData<TG>,
}

/// Watchdog driver
impl<TG> Wdt<TG>
where
    TG: TimerGroupInstance,
{
    /// Create a new watchdog timer instance
    pub fn new() -> Self {
        Self {
            phantom: PhantomData::default(),
        }
    }

    fn set_wdt_enabled(&mut self, enabled: bool) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .wdtwprotect
            .write(|w| unsafe { w.wdt_wkey().bits(0x50D8_3AA1u32) });

        if !enabled {
            reg_block.wdtconfig0.write(|w| unsafe { w.bits(0) });
        } else {
            reg_block.wdtconfig0.write(|w| w.wdt_en().bit(true));
        }

        reg_block
            .wdtwprotect
            .write(|w| unsafe { w.wdt_wkey().bits(0u32) });
    }

    fn feed(&mut self) {
        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .wdtwprotect
            .write(|w| unsafe { w.wdt_wkey().bits(0x50D8_3AA1u32) });

        reg_block.wdtfeed.write(|w| unsafe { w.bits(1) });

        reg_block
            .wdtwprotect
            .write(|w| unsafe { w.wdt_wkey().bits(0u32) });
    }

    fn set_timeout(&mut self, timeout: MicrosDurationU64) {
        let timeout_raw = (timeout.to_nanos() * 10 / 125) as u32;

        let reg_block = unsafe { &*TG::register_block() };

        reg_block
            .wdtwprotect
            .write(|w| unsafe { w.wdt_wkey().bits(0x50D8_3AA1u32) });

        reg_block
            .wdtconfig1
            .write(|w| unsafe { w.wdt_clk_prescale().bits(1) });

        reg_block
            .wdtconfig2
            .write(|w| unsafe { w.wdt_stg0_hold().bits(timeout_raw) });

        reg_block.wdtconfig0.write(|w| unsafe {
            w.wdt_en()
                .bit(true)
                .wdt_stg0()
                .bits(3)
                .wdt_cpu_reset_length()
                .bits(1)
                .wdt_sys_reset_length()
                .bits(1)
                .wdt_stg1()
                .bits(0)
                .wdt_stg2()
                .bits(0)
                .wdt_stg3()
                .bits(0)
        });

        #[cfg(feature = "esp32c3")]
        reg_block
            .wdtconfig0
            .modify(|_, w| w.wdt_conf_update_en().set_bit());

        reg_block
            .wdtwprotect
            .write(|w| unsafe { w.wdt_wkey().bits(0u32) });
    }
}

impl<TG> WatchdogDisable for Wdt<TG>
where
    TG: TimerGroupInstance,
{
    fn disable(&mut self) {
        self.set_wdt_enabled(false);
    }
}

impl<TG> WatchdogEnable for Wdt<TG>
where
    TG: TimerGroupInstance,
{
    type Time = MicrosDurationU64;

    fn start<T>(&mut self, period: T)
    where
        T: Into<Self::Time>,
    {
        self.set_timeout(period.into());
    }
}

impl<TG> Watchdog for Wdt<TG>
where
    TG: TimerGroupInstance,
{
    fn feed(&mut self) {
        self.feed();
    }
}
