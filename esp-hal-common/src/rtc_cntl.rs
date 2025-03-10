use embedded_hal::watchdog::{Watchdog, WatchdogDisable, WatchdogEnable};
use fugit::{HertzU32, MicrosDurationU64};

#[cfg(not(feature = "esp32"))]
use crate::efuse::Efuse;
use crate::{
    clock::{Clock, XtalClock},
    pac::{RTC_CNTL, TIMG0},
    rom::esp_rom_delay_us,
};

#[cfg_attr(feature = "esp32", path = "rtc/esp32.rs")]
#[cfg_attr(feature = "esp32s2", path = "rtc/esp32s2.rs")]
#[cfg_attr(feature = "esp32s3", path = "rtc/esp32s3.rs")]
#[cfg_attr(feature = "esp32c3", path = "rtc/esp32c3.rs")]
mod rtc;

#[allow(unused)]
#[derive(Debug, Clone, Copy)]
/// RTC SLOW_CLK frequency values
pub(crate) enum RtcFastClock {
    /// Main XTAL, divided by 4
    RtcFastClockXtalD4 = 0,
    /// Internal fast RC oscillator
    RtcFastClock8m     = 1,
}

impl Clock for RtcFastClock {
    fn frequency(&self) -> HertzU32 {
        match self {
            RtcFastClock::RtcFastClockXtalD4 => HertzU32::Hz(40_000_000 / 4),
            #[cfg(any(feature = "esp32", feature = "esp32s2"))]
            RtcFastClock::RtcFastClock8m => HertzU32::Hz(8_500_000),
            #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
            RtcFastClock::RtcFastClock8m => HertzU32::Hz(17_500_000),
        }
    }
}

#[allow(unused)]
#[derive(Debug, Clone, Copy)]
/// RTC SLOW_CLK frequency values
pub(crate) enum RtcSlowClock {
    /// Internal slow RC oscillator
    RtcSlowClockRtc     = 0,
    /// External 32 KHz XTAL
    RtcSlowClock32kXtal = 1,
    /// Internal fast RC oscillator, divided by 256
    RtcSlowClock8mD256  = 2,
}

impl Clock for RtcSlowClock {
    fn frequency(&self) -> HertzU32 {
        match self {
            #[cfg(feature = "esp32")]
            RtcSlowClock::RtcSlowClockRtc => HertzU32::Hz(150_000),
            #[cfg(feature = "esp32s2")]
            RtcSlowClock::RtcSlowClockRtc => HertzU32::Hz(90_000),
            #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
            RtcSlowClock::RtcSlowClockRtc => HertzU32::Hz(136_000),
            RtcSlowClock::RtcSlowClock32kXtal => HertzU32::Hz(32768),
            #[cfg(any(feature = "esp32", feature = "esp32s2"))]
            RtcSlowClock::RtcSlowClock8mD256 => HertzU32::Hz(8_500_000 / 256),
            #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
            RtcSlowClock::RtcSlowClock8mD256 => HertzU32::Hz(17_500_000 / 256),
        }
    }
}

#[allow(unused)]
#[derive(Debug, Clone, Copy)]
/// Clock source to be calibrated using rtc_clk_cal function
pub(crate) enum RtcCalSel {
    /// Currently selected RTC SLOW_CLK
    RtcCalRtcMux      = 0,
    /// Internal 8 MHz RC oscillator, divided by 256
    RtcCal8mD256      = 1,
    /// External 32 KHz XTAL
    RtcCal32kXtal     = 2,
    #[cfg(not(feature = "esp32"))]
    /// Internal 150 KHz RC oscillator
    RtcCalInternalOsc = 3,
}

pub struct Rtc {
    _inner: RTC_CNTL,
    pub rwdt: Rwdt,
    #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
    pub swd: Swd,
}

impl Rtc {
    pub fn new(rtc_cntl: RTC_CNTL) -> Self {
        rtc::init();
        rtc::configure_clock();

        Self {
            _inner: rtc_cntl,
            rwdt: Rwdt::default(),
            #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
            swd: Swd::new(),
        }
    }

    pub fn estimate_xtal_frequency(&mut self) -> u32 {
        RtcClock::estimate_xtal_frequency()
    }
}

/// RTC Watchdog Timer
pub struct RtcClock;
/// RTC Watchdog Timer driver
impl RtcClock {
    const CAL_FRACT: u32 = 19;

    /// Enable or disable 8 MHz internal oscillator
    ///
    /// Output from 8 MHz internal oscillator is passed into a configurable
    /// divider, which by default divides the input clock frequency by 256.
    /// Output of the divider may be used as RTC_SLOW_CLK source.
    /// Output of the divider is referred to in register descriptions and code
    /// as 8md256 or simply d256. Divider values other than 256 may be
    /// configured, but this facility is not currently needed, so is not
    /// exposed in the code.
    ///
    /// When 8MHz/256 divided output is not needed, the divider should be
    /// disabled to reduce power consumption.
    fn enable_8m(clk_8m_en: bool, d256_en: bool) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        if clk_8m_en {
            rtc_cntl.clk_conf.modify(|_, w| w.enb_ck8m().clear_bit());
            unsafe {
                rtc_cntl.timer1.modify(|_, w| w.ck8m_wait().bits(5));
                esp_rom_delay_us(50);
            }
        } else {
            rtc_cntl.clk_conf.modify(|_, w| w.enb_ck8m().set_bit());
            rtc_cntl
                .timer1
                .modify(|_, w| unsafe { w.ck8m_wait().bits(20) });
        }

        if d256_en {
            rtc_cntl
                .clk_conf
                .modify(|_, w| w.enb_ck8m_div().clear_bit());
        } else {
            rtc_cntl.clk_conf.modify(|_, w| w.enb_ck8m_div().set_bit());
        }
    }

    /// Get main XTAL frequency
    /// This is the value stored in RTC register RTC_XTAL_FREQ_REG by the
    /// bootloader, as passed to rtc_clk_init function.
    fn get_xtal_freq() -> XtalClock {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let xtal_freq_reg = rtc_cntl.store4.read().bits();

        // Values of RTC_XTAL_FREQ_REG and RTC_APB_FREQ_REG are stored as two copies in
        // lower and upper 16-bit halves. These are the routines to work with such a
        // representation.
        let clk_val_is_valid = |val| {
            (val & 0xffffu32) == ((val >> 16u32) & 0xffffu32) && val != 0u32 && val != u32::MAX
        };
        let reg_val_to_clk_val = |val| val & u16::MAX as u32;

        if !clk_val_is_valid(xtal_freq_reg) {
            return XtalClock::RtcXtalFreq40M;
        }

        match reg_val_to_clk_val(xtal_freq_reg) {
            40 => XtalClock::RtcXtalFreq40M,
            #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
            32 => XtalClock::RtcXtalFreq32M,
            #[cfg(feature = "esp32")]
            26 => XtalClock::RtcXtalFreq26M,
            #[cfg(feature = "esp32")]
            24 => XtalClock::RtcXtalFreq24M,
            other => XtalClock::RtcXtalFreqOther(other),
        }
    }

    /// Get the RTC_SLOW_CLK source
    fn get_slow_freq() -> RtcSlowClock {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let slow_freq = rtc_cntl.clk_conf.read().ana_clk_rtc_sel().bits();
        match slow_freq {
            0 => RtcSlowClock::RtcSlowClockRtc,
            1 => RtcSlowClock::RtcSlowClock32kXtal,
            2 => RtcSlowClock::RtcSlowClock8mD256,
            _ => unreachable!(),
        }
    }

    /// Select source for RTC_SLOW_CLK
    fn set_slow_freq(slow_freq: RtcSlowClock) {
        unsafe {
            let rtc_cntl = &*RTC_CNTL::ptr();
            rtc_cntl.clk_conf.modify(|_, w| {
                w.ana_clk_rtc_sel()
                    .bits(slow_freq as u8)
                    // Why we need to connect this clock to digital?
                    // Or maybe this clock should be connected to digital when
                    // XTAL 32k clock is enabled instead?
                    .dig_xtal32k_en()
                    .bit(match slow_freq {
                        RtcSlowClock::RtcSlowClock32kXtal => true,
                        _ => false,
                    })
                    // The clk_8m_d256 will be closed when rtc_state in SLEEP,
                    // so if the slow_clk is 8md256, clk_8m must be force power on
                    .ck8m_force_pu()
                    .bit(match slow_freq {
                        RtcSlowClock::RtcSlowClock8mD256 => true,
                        _ => false,
                    })
            });

            esp_rom_delay_us(300u32);
        };
    }

    /// Select source for RTC_FAST_CLK
    fn set_fast_freq(fast_freq: RtcFastClock) {
        unsafe {
            let rtc_cntl = &*RTC_CNTL::ptr();
            rtc_cntl.clk_conf.modify(|_, w| {
                w.fast_clk_rtc_sel().bit(match fast_freq {
                    RtcFastClock::RtcFastClock8m => true,
                    RtcFastClock::RtcFastClockXtalD4 => false,
                })
            });

            esp_rom_delay_us(3u32);
        };
    }

    /// Calibration of RTC_SLOW_CLK is performed using a special feature of
    /// TIMG0. This feature counts the number of XTAL clock cycles within a
    /// given number of RTC_SLOW_CLK cycles.
    fn calibrate_internal(cal_clk: RtcCalSel, slowclk_cycles: u32) -> u32 {
        // Except for ESP32, choosing RTC_CAL_RTC_MUX results in calibration of
        // the 150k RTC clock (90k on ESP32-S2) regardless of the currently selected
        // SLOW_CLK. On the ESP32, it uses the currently selected SLOW_CLK.
        // The following code emulates ESP32 behavior for the other chips:
        #[cfg(not(feature = "esp32"))]
        let cal_clk = match cal_clk {
            RtcCalSel::RtcCalRtcMux => match RtcClock::get_slow_freq() {
                RtcSlowClock::RtcSlowClock32kXtal => RtcCalSel::RtcCal32kXtal,
                RtcSlowClock::RtcSlowClock8mD256 => RtcCalSel::RtcCal8mD256,
                _ => cal_clk,
            },
            RtcCalSel::RtcCalInternalOsc => RtcCalSel::RtcCalRtcMux,
            _ => cal_clk,
        };
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let timg0 = unsafe { &*TIMG0::ptr() };

        // Enable requested clock (150k clock is always on)
        let dig_32k_xtal_enabled = rtc_cntl.clk_conf.read().dig_xtal32k_en().bit_is_set();

        if matches!(cal_clk, RtcCalSel::RtcCal32kXtal) && !dig_32k_xtal_enabled {
            rtc_cntl
                .clk_conf
                .modify(|_, w| w.dig_xtal32k_en().set_bit());
        }

        if matches!(cal_clk, RtcCalSel::RtcCal8mD256) {
            rtc_cntl
                .clk_conf
                .modify(|_, w| w.dig_clk8m_d256_en().set_bit());
        }

        // There may be another calibration process already running during we
        // call this function, so we should wait the last process is done.
        #[cfg(not(feature = "esp32"))]
        if timg0
            .rtccalicfg
            .read()
            .rtc_cali_start_cycling()
            .bit_is_set()
        {
            // Set a small timeout threshold to accelerate the generation of timeout.
            // The internal circuit will be reset when the timeout occurs and will not
            // affect the next calibration.
            timg0
                .rtccalicfg2
                .modify(|_, w| unsafe { w.rtc_cali_timeout_thres().bits(1) });

            while timg0.rtccalicfg.read().rtc_cali_rdy().bit_is_clear()
                && timg0.rtccalicfg2.read().rtc_cali_timeout().bit_is_clear()
            {}
        }

        // Prepare calibration
        timg0.rtccalicfg.modify(|_, w| unsafe {
            w.rtc_cali_clk_sel()
                .bits(cal_clk as u8)
                .rtc_cali_start_cycling()
                .clear_bit()
                .rtc_cali_max()
                .bits(slowclk_cycles as u16)
        });

        // Figure out how long to wait for calibration to finish
        // Set timeout reg and expect time delay
        let expected_freq = match cal_clk {
            RtcCalSel::RtcCal32kXtal => {
                #[cfg(not(feature = "esp32"))]
                timg0.rtccalicfg2.modify(|_, w| unsafe {
                    w.rtc_cali_timeout_thres().bits(slowclk_cycles << 12)
                });
                RtcSlowClock::RtcSlowClock32kXtal
            }
            RtcCalSel::RtcCal8mD256 => {
                #[cfg(not(feature = "esp32"))]
                timg0.rtccalicfg2.modify(|_, w| unsafe {
                    w.rtc_cali_timeout_thres().bits(slowclk_cycles << 12)
                });
                RtcSlowClock::RtcSlowClock8mD256
            }
            _ => {
                #[cfg(not(feature = "esp32"))]
                timg0.rtccalicfg2.modify(|_, w| unsafe {
                    w.rtc_cali_timeout_thres().bits(slowclk_cycles << 10)
                });
                RtcSlowClock::RtcSlowClockRtc
            }
        };

        let us_time_estimate = HertzU32::MHz(slowclk_cycles) / expected_freq.frequency();

        // Start calibration
        timg0
            .rtccalicfg
            .modify(|_, w| w.rtc_cali_start().clear_bit().rtc_cali_start().set_bit());

        // Wait for calibration to finish up to another us_time_estimate
        unsafe {
            esp_rom_delay_us(us_time_estimate);
        }

        #[cfg(feature = "esp32")]
        let mut timeout_us = us_time_estimate;

        let cal_val = loop {
            if timg0.rtccalicfg.read().rtc_cali_rdy().bit_is_set() {
                break timg0.rtccalicfg1.read().rtc_cali_value().bits();
            }

            #[cfg(not(feature = "esp32"))]
            if timg0.rtccalicfg2.read().rtc_cali_timeout().bit_is_set() {
                // Timed out waiting for calibration
                break 0;
            }

            #[cfg(feature = "esp32")]
            if timeout_us > 0 {
                timeout_us -= 1;
                unsafe {
                    esp_rom_delay_us(1);
                }
            } else {
                // Timed out waiting for calibration
                break 0;
            }
        };

        timg0
            .rtccalicfg
            .modify(|_, w| w.rtc_cali_start().clear_bit());
        rtc_cntl
            .clk_conf
            .modify(|_, w| w.dig_xtal32k_en().bit(dig_32k_xtal_enabled));

        if matches!(cal_clk, RtcCalSel::RtcCal8mD256) {
            rtc_cntl
                .clk_conf
                .modify(|_, w| w.dig_clk8m_d256_en().clear_bit());
        }

        cal_val
    }

    /// Measure ratio between XTAL frequency and RTC slow clock frequency
    fn get_calibration_ratio(cal_clk: RtcCalSel, slowclk_cycles: u32) -> u32 {
        let xtal_cycles = RtcClock::calibrate_internal(cal_clk, slowclk_cycles) as u64;
        let ratio = (xtal_cycles << RtcClock::CAL_FRACT) / slowclk_cycles as u64;

        (ratio & (u32::MAX as u64)) as u32
    }

    /// Measure RTC slow clock's period, based on main XTAL frequency
    ///
    /// This function will time out and return 0 if the time for the given
    /// number of cycles to be counted exceeds the expected time twice. This
    /// may happen if 32k XTAL is being calibrated, but the oscillator has
    /// not started up (due to incorrect loading capacitance, board design
    /// issue, or lack of 32 XTAL on board).
    fn calibrate(cal_clk: RtcCalSel, slowclk_cycles: u32) -> u32 {
        let xtal_freq = RtcClock::get_xtal_freq();
        let xtal_cycles = RtcClock::calibrate_internal(cal_clk, slowclk_cycles) as u64;
        let divider = xtal_freq.mhz() as u64 * slowclk_cycles as u64;
        let period_64 = ((xtal_cycles << RtcClock::CAL_FRACT) + divider / 2u64 - 1u64) / divider;

        (period_64 & u32::MAX as u64) as u32
    }

    /// Calculate the necessary RTC_SLOW_CLK cycles to complete 1 millisecond.
    fn cycles_to_1ms() -> u16 {
        let period_13q19 = RtcClock::calibrate(
            match RtcClock::get_slow_freq() {
                RtcSlowClock::RtcSlowClockRtc => RtcCalSel::RtcCalRtcMux,
                RtcSlowClock::RtcSlowClock32kXtal => RtcCalSel::RtcCal32kXtal,
                RtcSlowClock::RtcSlowClock8mD256 => RtcCalSel::RtcCal8mD256,
            },
            1024,
        );

        let q_to_float = |val| (val as f32) / ((1 << RtcClock::CAL_FRACT) as f32);
        let period = q_to_float(period_13q19);

        (1000f32 / period) as u16
    }

    fn estimate_xtal_frequency() -> u32 {
        // Number of 8M/256 clock cycles to use for XTAL frequency estimation.
        const XTAL_FREQ_EST_CYCLES: u32 = 10;

        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let clk_8m_enabled = rtc_cntl.clk_conf.read().enb_ck8m().bit_is_clear();
        let clk_8md256_enabled = rtc_cntl.clk_conf.read().enb_ck8m_div().bit_is_clear();

        if !clk_8md256_enabled {
            RtcClock::enable_8m(true, true);
        }

        let ratio = RtcClock::get_calibration_ratio(RtcCalSel::RtcCal8mD256, XTAL_FREQ_EST_CYCLES);
        let freq_mhz =
            ((ratio as u64 * RtcFastClock::RtcFastClock8m.hz() as u64 / 1_000_000u64 / 256u64)
                >> RtcClock::CAL_FRACT) as u32;

        RtcClock::enable_8m(clk_8m_enabled, clk_8md256_enabled);

        freq_mhz
    }
}

/// Behavior of the RWDT stage if it times out
#[allow(unused)]
#[derive(Debug, Clone, Copy)]
enum RwdtStageAction {
    RwdtStageActionOff         = 0,
    RwdtStageActionInterrupt   = 1,
    RwdtStageActionResetCpu    = 2,
    RwdtStageActionResetSystem = 3,
    RwdtStageActionResetRtc    = 4,
}

/// RTC Watchdog Timer
pub struct Rwdt {
    stg0_action: RwdtStageAction,
    stg1_action: RwdtStageAction,
    stg2_action: RwdtStageAction,
    stg3_action: RwdtStageAction,
}

impl Default for Rwdt {
    fn default() -> Self {
        Self {
            stg0_action: RwdtStageAction::RwdtStageActionResetRtc,
            stg1_action: RwdtStageAction::RwdtStageActionOff,
            stg2_action: RwdtStageAction::RwdtStageActionOff,
            stg3_action: RwdtStageAction::RwdtStageActionOff,
        }
    }
}

/// RTC Watchdog Timer driver
impl Rwdt {
    pub fn listen(&mut self) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        self.stg0_action = RwdtStageAction::RwdtStageActionInterrupt;

        self.set_write_protection(false);

        // Configure STAGE0 to trigger an interrupt upon expiration
        rtc_cntl
            .wdtconfig0
            .modify(|_, w| unsafe { w.wdt_stg0().bits(self.stg0_action as u8) });

        #[cfg(feature = "esp32")]
        rtc_cntl.int_ena.modify(|_, w| w.wdt_int_ena().set_bit());

        #[cfg(feature = "esp32s2")]
        rtc_cntl
            .int_ena_rtc
            .modify(|_, w| w.wdt_int_ena().set_bit());

        #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
        rtc_cntl
            .int_ena_rtc
            .modify(|_, w| w.rtc_wdt_int_ena().set_bit());

        self.set_write_protection(true);
    }

    pub fn unlisten(&mut self) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        self.stg0_action = RwdtStageAction::RwdtStageActionResetRtc;

        self.set_write_protection(false);

        // Configure STAGE0 to reset the main system and the RTC upon expiration.
        rtc_cntl
            .wdtconfig0
            .modify(|_, w| unsafe { w.wdt_stg0().bits(self.stg0_action as u8) });

        #[cfg(feature = "esp32")]
        rtc_cntl.int_ena.modify(|_, w| w.wdt_int_ena().clear_bit());

        #[cfg(feature = "esp32s2")]
        rtc_cntl
            .int_ena_rtc
            .modify(|_, w| w.wdt_int_ena().clear_bit());

        #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
        rtc_cntl
            .int_ena_rtc
            .modify(|_, w| w.rtc_wdt_int_ena().clear_bit());

        self.set_write_protection(true);
    }

    pub fn clear_interrupt(&mut self) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        self.set_write_protection(false);

        #[cfg(feature = "esp32")]
        rtc_cntl.int_clr.write(|w| w.wdt_int_clr().set_bit());

        #[cfg(feature = "esp32s2")]
        rtc_cntl.int_clr_rtc.write(|w| w.wdt_int_clr().set_bit());

        #[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
        rtc_cntl
            .int_clr_rtc
            .write(|w| w.rtc_wdt_int_clr().set_bit());

        self.set_write_protection(true);
    }

    pub fn is_interrupt_set(&self) -> bool {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        cfg_if::cfg_if! {
            if #[cfg(feature = "esp32")] {
                rtc_cntl.int_st.read().wdt_int_st().bit_is_set()
            } else if #[cfg(feature = "esp32s2")] {
                rtc_cntl.int_st_rtc.read().wdt_int_st().bit_is_set()
            } else if #[cfg(any(feature = "esp32c3", feature = "esp32s3"))] {
                rtc_cntl.int_st_rtc.read().rtc_wdt_int_st().bit_is_set()
            }
        }
    }

    /// Enable/disable write protection for WDT registers
    fn set_write_protection(&mut self, enable: bool) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let wkey = if enable { 0u32 } else { 0x50D8_3AA1 };

        rtc_cntl.wdtwprotect.write(|w| unsafe { w.bits(wkey) });
    }
}

impl WatchdogDisable for Rwdt {
    fn disable(&mut self) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        self.set_write_protection(false);

        rtc_cntl
            .wdtconfig0
            .modify(|_, w| w.wdt_en().clear_bit().wdt_flashboot_mod_en().clear_bit());

        self.set_write_protection(true);
    }
}

impl WatchdogEnable for Rwdt {
    type Time = MicrosDurationU64;

    fn start<T>(&mut self, period: T)
    where
        T: Into<Self::Time>,
    {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let timeout_raw = (period.into().to_millis() * (RtcClock::cycles_to_1ms() as u64)) as u32;

        self.set_write_protection(false);

        unsafe {
            #[cfg(feature = "esp32")]
            rtc_cntl
                .wdtconfig1
                .modify(|_, w| w.wdt_stg0_hold().bits(timeout_raw));

            #[cfg(not(feature = "esp32"))]
            rtc_cntl.wdtconfig1.modify(|_, w| {
                w.wdt_stg0_hold()
                    .bits(timeout_raw >> (1 + Efuse::get_rwdt_multiplier()))
            });

            rtc_cntl.wdtconfig0.modify(|_, w| {
                w.wdt_stg0()
                    .bits(self.stg0_action as u8)
                    .wdt_cpu_reset_length()
                    .bits(7)
                    .wdt_sys_reset_length()
                    .bits(7)
                    .wdt_stg1()
                    .bits(self.stg1_action as u8)
                    .wdt_stg2()
                    .bits(self.stg2_action as u8)
                    .wdt_stg3()
                    .bits(self.stg3_action as u8)
                    .wdt_en()
                    .set_bit()
            });
        }

        self.set_write_protection(true);
    }
}

impl Watchdog for Rwdt {
    fn feed(&mut self) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        self.set_write_protection(false);

        rtc_cntl.wdtfeed.write(|w| unsafe { w.bits(1) });

        self.set_write_protection(true);
    }
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
/// Super Watchdog
pub struct Swd;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
/// Super Watchdog driver
impl Swd {
    pub fn new() -> Self {
        Self
    }

    /// Enable/disable write protection for WDT registers
    fn set_write_protection(&mut self, enable: bool) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };
        let wkey = if enable { 0u32 } else { 0x8F1D_312A };

        rtc_cntl
            .swd_wprotect
            .write(|w| unsafe { w.swd_wkey().bits(wkey) });
    }
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
impl WatchdogDisable for Swd {
    fn disable(&mut self) {
        let rtc_cntl = unsafe { &*RTC_CNTL::ptr() };

        self.set_write_protection(false);

        rtc_cntl.swd_conf.write(|w| w.swd_auto_feed_en().set_bit());

        self.set_write_protection(true);
    }
}
