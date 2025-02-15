//! Checks the CDP payload. Uses the [ItsPayloadFsmContinuous] state machine to determine which words to expect.
//!
//! [CdpRunningValidator] delegates sanity checks to word specific sanity checkers.
use super::data_words::DATA_WORD_SANITY_CHECKER;
use crate::util::lib::Config;
use crate::validators::its_payload_fsm_cont::ItsPayloadFsmContinuous;
use crate::validators::its_payload_fsm_cont::PayloadWord;
use crate::words::data_words::{
    ob_data_word_id_to_input_number_connector, ob_data_word_id_to_lane,
};
use crate::words::lib::RDH;
use crate::words::status_words::{is_lane_active, Cdw};
use crate::{
    stats::stats_controller::StatType,
    validators::status_words::STATUS_WORD_SANITY_CHECKER,
    words::status_words::{Ddw0, Ihw, StatusWord, Tdh, Tdt},
};

enum StatusWordKind<'a> {
    Ihw(&'a [u8]),
    Tdh(&'a [u8]),
    Tdt(&'a [u8]),
    Ddw0(&'a [u8]),
}

struct CdpRunningLocalConfig {
    running_checks: bool,
}

impl CdpRunningLocalConfig {
    fn new(config: &impl crate::util::lib::Checks) -> Self {
        use crate::util::config::Check;
        match config.check() {
            Some(Check::All(_)) => Self {
                running_checks: true,
            },
            _ => Self {
                running_checks: false,
            },
        }
    }
}

/// Checks the CDP payload and reports any errors.
pub struct CdpRunningValidator<T: RDH> {
    config: CdpRunningLocalConfig,
    its_state_machine: ItsPayloadFsmContinuous,
    current_rdh: Option<T>,
    current_ihw: Option<Ihw>,
    current_tdh: Option<Tdh>,
    previous_tdh: Option<Tdh>,
    current_tdt: Option<Tdt>,
    current_ddw0: Option<Ddw0>,
    previous_cdw: Option<Cdw>,
    gbt_word_counter: u16,
    pub(crate) stats_send_ch: std::sync::mpsc::Sender<StatType>,
    payload_mem_pos: u64,
    gbt_word_padding_size_bytes: u8,
    is_new_data: bool, // Flag used to indicate start of new CDP payload where a CDW is valid
}

impl<T: RDH> Default for CdpRunningValidator<T> {
    fn default() -> Self {
        Self {
            config: CdpRunningLocalConfig {
                running_checks: false,
            },
            its_state_machine: ItsPayloadFsmContinuous::default(),
            current_rdh: None,
            current_ihw: None,
            current_tdh: None,
            previous_tdh: None,
            current_tdt: None,
            current_ddw0: None,
            previous_cdw: None,
            gbt_word_counter: 0,
            stats_send_ch: std::sync::mpsc::channel().0,
            payload_mem_pos: 0,
            gbt_word_padding_size_bytes: 0,
            is_new_data: false,
        }
    }
}

impl<T: RDH> CdpRunningValidator<T> {
    /// Creates a new [CdpRunningValidator] from a [Config] and a [StatType] producer channel.
    pub fn new(config: &impl Config, stats_send_ch: std::sync::mpsc::Sender<StatType>) -> Self {
        Self {
            config: CdpRunningLocalConfig::new(config),
            its_state_machine: ItsPayloadFsmContinuous::default(),
            current_rdh: None,
            current_ihw: None,
            current_tdh: None,
            previous_tdh: None,
            current_tdt: None,
            current_ddw0: None,
            previous_cdw: None,
            gbt_word_counter: 0,
            stats_send_ch,
            payload_mem_pos: 0,
            gbt_word_padding_size_bytes: 0,
            is_new_data: false,
        }
    }

    // For testing configs
    #[allow(dead_code)]
    fn set_config(&mut self, config: &impl crate::util::lib::Checks) {
        self.config = CdpRunningLocalConfig::new(config);
    }

    /// Helper function to format and report an error
    ///
    /// Takes in the error string slice and the word slice
    /// Adds the current memory position to the error string
    /// Sends the error to the stats channel
    #[inline]
    fn report_error(&self, error: &str, word_slice: &[u8]) {
        let mem_pos = self.calc_current_word_mem_pos();
        self.stats_send_ch
            .send(StatType::Error(format!(
                "{mem_pos:#X}: {error} [{:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X}]",
                word_slice[0],
                word_slice[1],
                word_slice[2],
                word_slice[3],
                word_slice[4],
                word_slice[5],
                word_slice[6],
                word_slice[7],
                word_slice[8],
                word_slice[9],
                            )))
            .expect("Failed to send error to stats channel");
    }

    /// Resets the state machine to the initial state and logs a warning
    ///
    /// Use this if a payload format is invalid and the next payload can be processed from the initial state
    #[inline]
    pub fn reset_fsm(&mut self) {
        log::warn!("Resetting CDP Payload FSM");
        self.its_state_machine.reset_fsm();
    }

    /// This function has to be called for every RDH
    ///
    /// It defines what is valid, and is necessary to keep track of the memory position of each word
    /// It uses the RDH to determine size of padding
    #[inline]
    pub fn set_current_rdh(&mut self, rdh: &T, rdh_mem_pos: u64) {
        self.current_rdh = Some(T::load(&mut rdh.to_byte_slice()).unwrap());
        self.payload_mem_pos = rdh_mem_pos + 64;
        if rdh.data_format() == 0 {
            self.gbt_word_padding_size_bytes = 6; // Data format 0
        } else {
            self.gbt_word_padding_size_bytes = 0; // Data format 2
        }
        self.is_new_data = true;
        self.gbt_word_counter = 0;
    }

    /// This function has to be called for every GBT word
    #[inline]
    pub fn check(&mut self, gbt_word: &[u8]) {
        debug_assert!(gbt_word.len() == 10);
        self.gbt_word_counter += 1; // Tracks the number of GBT words seen in the current CDP

        let current_word = self.its_state_machine.advance(gbt_word);

        match current_word {
            PayloadWord::IHW => {
                self.process_status_word(StatusWordKind::Ihw(gbt_word));
                self.check_rdh_at_initial_ihw(gbt_word);
            }
            PayloadWord::IHW_continuation => {
                self.process_status_word(StatusWordKind::Ihw(gbt_word))
            }
            PayloadWord::TDH => {
                self.process_status_word(StatusWordKind::Tdh(gbt_word));
                self.check_tdh_no_continuation(gbt_word);
            }
            PayloadWord::TDH_continuation => {
                self.process_status_word(StatusWordKind::Tdh(gbt_word));
                self.check_tdh_continuation(gbt_word);
            }
            PayloadWord::TDH_after_packet_done => {
                self.process_status_word(StatusWordKind::Tdh(gbt_word));
                self.check_tdh_by_was_tdt_packet_done_true(gbt_word);
            }
            PayloadWord::TDT => self.process_status_word(StatusWordKind::Tdt(gbt_word)),
            // DataWord and CDW are handled together
            PayloadWord::CDW | PayloadWord::DataWord => self.process_data_word(gbt_word),

            PayloadWord::DDW0 => self.process_status_word(StatusWordKind::Ddw0(gbt_word)),
        }
    }

    /// Calculates the current position in the memory of the current word.
    ///
    /// Current payload position is the first byte after the current RDH
    /// The gbt word position then relative to the current payload is then:
    /// relative_mem_pos = gbt_word_counter * (10 + gbt_word_padding_size_bytes)
    /// And the absolute position in the memory is then:
    /// gbt_word_mem_pos = payload_mem_pos + relative_mem_pos
    #[inline]
    fn calc_current_word_mem_pos(&self) -> u64 {
        let gbt_word_memory_size_bytes: u64 = 10 + self.gbt_word_padding_size_bytes as u64;
        let relative_mem_pos = (self.gbt_word_counter - 1) as u64 * gbt_word_memory_size_bytes;
        relative_mem_pos + self.payload_mem_pos
    }

    /// Takes a slice of bytes wrapped in an enum of the expected status word then:
    /// 1. Deserializes the slice as the expected status word and checks it for sanity.
    /// 2. If the sanity check fails, the error is sent to the stats channel
    /// 3. Stores the deserialized status word as the last status word of the same type.
    #[inline]
    fn process_status_word(&mut self, status_word: StatusWordKind) {
        match status_word {
            StatusWordKind::Ihw(ihw_as_slice) => {
                let ihw = Ihw::load(&mut <&[u8]>::clone(&ihw_as_slice)).unwrap();
                log::debug!("{ihw}");
                if let Err(e) = STATUS_WORD_SANITY_CHECKER.sanity_check_ihw(&ihw) {
                    self.report_error(&format!("[E30] {e}"), ihw_as_slice);
                }
                self.current_ihw = Some(ihw);
            }
            StatusWordKind::Tdh(tdh_as_slice) => {
                let tdh = Tdh::load(&mut <&[u8]>::clone(&tdh_as_slice)).unwrap();
                log::debug!("{tdh}");
                if let Err(e) = STATUS_WORD_SANITY_CHECKER.sanity_check_tdh(&tdh) {
                    self.report_error(&format!("[E40] {e}"), tdh_as_slice);
                }
                // Swap current and last TDH, then replace current with the new TDH
                std::mem::swap(&mut self.current_tdh, &mut self.previous_tdh);
                self.current_tdh = Some(tdh);
            }
            StatusWordKind::Tdt(tdt_as_slice) => {
                let tdt = Tdt::load(&mut <&[u8]>::clone(&tdt_as_slice)).unwrap();
                log::debug!("{tdt}");
                if let Err(e) = STATUS_WORD_SANITY_CHECKER.sanity_check_tdt(&tdt) {
                    self.report_error(&format!("[E50] {e}"), tdt_as_slice);
                }
                self.current_tdt = Some(tdt);
            }
            StatusWordKind::Ddw0(ddw0_as_slice) => {
                let ddw0 = Ddw0::load(&mut <&[u8]>::clone(&ddw0_as_slice)).unwrap();
                log::debug!("{ddw0}");
                if let Err(e) = STATUS_WORD_SANITY_CHECKER.sanity_check_ddw0(&ddw0) {
                    self.report_error(&format!("[E60] {e}"), ddw0_as_slice);
                }

                // Additional state dependent checks on RDH
                self.check_rdh_at_ddw0(ddw0_as_slice);
                self.current_ddw0 = Some(ddw0);
            }
        }
    }

    /// Takes a slice of bytes expected to be a data word, and checks if it has a valid identifier.
    #[inline]
    fn process_data_word(&mut self, data_word_slice: &[u8]) {
        let id_index = 9;
        if self.is_new_data && data_word_slice[id_index] == 0xF8 {
            // CDW
            self.process_cdw(data_word_slice);
        } else {
            // Regular data word
            if let Err(e) = DATA_WORD_SANITY_CHECKER.check_any(data_word_slice) {
                self.report_error(&format!("[E70] {e}"), data_word_slice);
                log::debug!("Data word: {data_word_slice:?}");
            }
            let id_3_msb = data_word_slice[id_index] >> 5;
            if id_3_msb == 0b001 {
                // Inner Barrel
                self.process_ib_data_word(data_word_slice);
            } else if id_3_msb == 0b010 {
                // Outer Barrel
                self.process_ob_data_word(data_word_slice);
            }
        }

        self.is_new_data = false;
    }

    #[inline]
    fn process_ib_data_word(&mut self, ib_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        let lane_id = ib_slice[9] & 0x1F;
        // lane in active_lanes
        let active_lanes = self.current_ihw.as_ref().unwrap().active_lanes();
        if !is_lane_active(lane_id, active_lanes) {
            self.report_error(
                &format!("[E72] IB lane {lane_id} is not active according to IHW active_lanes: {active_lanes:#X}."),
                ib_slice,
            );
        }
    }

    #[inline]
    fn process_ob_data_word(&mut self, ob_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        let lane_id = ob_data_word_id_to_lane(ob_slice[9]);
        // lane in active_lanes
        let active_lanes = self.current_ihw.as_ref().unwrap().active_lanes();
        if !is_lane_active(lane_id, active_lanes) {
            self.report_error(
                &format!("[E71] OB lane {lane_id} is not active according to IHW active_lanes: {active_lanes:#X}."),
                ob_slice,
            );
        }

        // lane in connector <= 6
        let input_number_connector = ob_data_word_id_to_input_number_connector(ob_slice[9]);
        if input_number_connector > 6 {
            self.report_error(
                &format!("[E73] OB Data Word has input connector {input_number_connector} > 6."),
                ob_slice,
            );
        }
    }

    #[inline]
    fn process_cdw(&mut self, cdw_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        let cdw = Cdw::load(&mut <&[u8]>::clone(&cdw_slice)).unwrap();
        log::debug!("{cdw}");

        if let Some(previous_cdw) = self.previous_cdw.as_ref() {
            if previous_cdw.calibration_user_fields() != cdw.calibration_user_fields()
                && cdw.calibration_word_index() != 0
            {
                self.report_error("[E81] CDW index is not 0", cdw_slice);
            }
        }

        self.previous_cdw = Some(cdw);
    }

    // Minor checks done in certain states

    /// Checks TDH trigger and continuation following a TDT packet_done = 1
    #[inline]
    fn check_tdh_by_was_tdt_packet_done_true(&mut self, tdh_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        if self.current_tdh.as_ref().unwrap().internal_trigger() != 1 {
            self.report_error("[E43] TDH internal trigger is not 1", tdh_slice);
            let tmp_rdh = self.current_rdh.as_ref().unwrap();
            log::debug!("{tmp_rdh}");
        }
        if let Some(previous_tdh) = self.previous_tdh.as_ref() {
            if previous_tdh.trigger_bc() > self.current_tdh.as_ref().unwrap().trigger_bc() {
                self.report_error(
                    &format!(
                        "[E44] TDH trigger_bc is not increasing, previous: {:#X}, current: {:#X}.",
                        previous_tdh.trigger_bc(),
                        self.current_tdh.as_ref().unwrap().trigger_bc()
                    ),
                    tdh_slice,
                );
            }
        }
    }

    /// Checks RDH stop_bit and pages_counter when a DDW0 is observed
    #[inline]
    fn check_rdh_at_ddw0(&mut self, ddw0_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        if self.current_rdh.as_ref().unwrap().stop_bit() != 1 {
            self.report_error("[E11] DDW0 observed but RDH stop bit is not 1", ddw0_slice);
        }
        if self.current_rdh.as_ref().unwrap().pages_counter() == 0 {
            self.report_error("[E11] DDW0 observed but RDH page counter is 0", ddw0_slice);
        }
    }
    /// Checks RDH stop_bit and pages_counter when an initial IHW is observed (not IHW during continuation)
    #[inline]
    fn check_rdh_at_initial_ihw(&mut self, ihw_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        if self.current_rdh.as_ref().unwrap().stop_bit() != 0 {
            self.report_error("[E12] IHW observed but RDH stop bit is not 0", ihw_slice);
        }
    }

    /// Checks TDH when continuation is expected (Previous TDT packet_done = 0)
    #[inline]
    fn check_tdh_continuation(&mut self, tdh_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        if self.current_tdh.as_ref().unwrap().continuation() != 1 {
            self.report_error("[E41] TDH continuation is not 1", tdh_slice);
        }

        if let Some(previous_tdh) = self.previous_tdh.as_ref() {
            if previous_tdh.trigger_bc() != self.current_tdh.as_ref().unwrap().trigger_bc() {
                self.report_error("[E44] TDH trigger_bc is not the same", tdh_slice);
            }
            if previous_tdh.trigger_orbit != self.current_tdh.as_ref().unwrap().trigger_orbit {
                self.report_error("[E44] TDH trigger_orbit is not the same", tdh_slice);
            }
            if previous_tdh.trigger_type() != self.current_tdh.as_ref().unwrap().trigger_type() {
                self.report_error("[E44] TDH trigger_type is not the same", tdh_slice);
            }
        }
    }

    /// Checks TDH continuation, orbit when the TDH immediately follows an IHW
    #[inline]
    fn check_tdh_no_continuation(&mut self, tdh_slice: &[u8]) {
        if !self.config.running_checks {
            return;
        }
        let current_rdh = self.current_rdh.as_ref().expect("RDH should be set");
        let current_tdh = self
            .current_tdh
            .as_ref()
            .expect("TDH should be set, process words before checks");

        if current_tdh.continuation() != 0 {
            self.report_error("[E42] TDH continuation is not 0", tdh_slice);
        }

        if current_tdh.trigger_orbit != current_rdh.rdh1().orbit {
            self.report_error(
                "[E44] TDH trigger_orbit is not equal to RDH orbit",
                tdh_slice,
            );
        }

        if current_rdh.pages_counter() == 0
            && (current_tdh.internal_trigger() == 1 || current_rdh.rdh2().is_pht_trigger())
        {
            // In this case the bc and trigger_type of the TDH and RDH should match
            if current_rdh.rdh1().bc() != current_tdh.trigger_bc() {
                self.report_error(
                    &format!(
                        "[E44] TDH trigger_bc is not equal to RDH bc, TDH: {:#X}, RDH: {:#X}.",
                        current_tdh.trigger_bc(),
                        current_rdh.rdh1().bc()
                    ),
                    tdh_slice,
                );
            }
            // TDH only has the 12 LSB of the trigger type
            if current_rdh.rdh2().trigger_type as u16 & 0xFFF != current_tdh.trigger_type() {
                let tmp_rdh_trig = current_rdh.rdh2().trigger_type as u16;
                self.report_error(
                        &format!("[E44] TDH trigger_type is not equal to RDH trigger_type, TDH: {:#X}, RDH: {tmp_rdh_trig:#X}", current_tdh.trigger_type()),
                        tdh_slice,
                    );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        util::config::Target,
        util::lib::MockChecks,
        words::rdh_cru::{test_data::CORRECT_RDH_CRU_V7, RdhCRU, V7},
    };
    #[test]
    fn test_validate_ihw() {
        const VALID_ID: u8 = 0xE0;
        const _ACTIVE_LANES_14_ACTIVE: u32 = 0x3F_FF;
        let raw_data_ihw = [
            0xFF, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, VALID_ID,
        ];

        let (send, stats_recv_ch) = std::sync::mpsc::channel();
        let mut validator = CdpRunningValidator::<RdhCRU<V7>>::default();
        validator.stats_send_ch = send;
        let rdh_mem_pos = 0;

        validator.set_current_rdh(&CORRECT_RDH_CRU_V7, rdh_mem_pos);
        validator.check(&raw_data_ihw);

        assert!(stats_recv_ch.try_recv().is_err()); // Checks that no error was received (nothing received)
    }

    #[test]
    fn test_invalidate_ihw() {
        const INVALID_ID: u8 = 0xE1;
        const _ACTIVE_LANES_14_ACTIVE: u32 = 0x3F_FF;
        let raw_data_ihw = [
            0xFF, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, INVALID_ID,
        ];

        let (send, stats_recv_ch) = std::sync::mpsc::channel();
        let mut validator = CdpRunningValidator::<RdhCRU<V7>>::default();
        validator.stats_send_ch = send;
        let rdh_mem_pos = 0x0;

        validator.set_current_rdh(&CORRECT_RDH_CRU_V7, rdh_mem_pos);
        validator.check(&raw_data_ihw);

        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x40: [E30] ID is not 0xE0: 0xE1  [FF 3F 00 00 00 00 00 00 00 E1]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_expect_ihw_invalidate_tdh() {
        const _VALID_ID: u8 = 0xF0;
        // Boring but very typical TDT, everything is 0 except for packet_done
        let raw_data_tdt = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF1];

        let (send, stats_recv_ch) = std::sync::mpsc::channel();
        let mut validator = CdpRunningValidator::<RdhCRU<V7>>::default();
        validator.stats_send_ch = send;
        let rdh_mem_pos = 0x0; // RDH size is 64 bytes

        validator.set_current_rdh(&CORRECT_RDH_CRU_V7, rdh_mem_pos); // Data format is 2
        validator.check(&raw_data_tdt);

        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x40: [E30] ID is not 0xE0: 0xF1  [00 00 00 00 00 00 00 00 01 F1]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_expect_ihw_invalidate_tdh_and_next() {
        const _VALID_ID: u8 = 0xF0;
        // Boring but very typical TDT, everything is 0 except for packet_done
        let raw_data_tdt = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF1];
        let raw_data_tdt_next = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF2];

        let (send, stats_recv_ch) = std::sync::mpsc::channel();
        let mut validator = CdpRunningValidator::<RdhCRU<V7>>::default();
        validator.stats_send_ch = send;
        let rdh_mem_pos = 0x0; // RDH size is 64 bytes

        validator.set_current_rdh(&CORRECT_RDH_CRU_V7, rdh_mem_pos); // Data format is 2
        validator.check(&raw_data_tdt);
        validator.check(&raw_data_tdt_next);

        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x40: [E30] ID is not 0xE0: 0xF1  [00 00 00 00 00 00 00 00 01 F1]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x4A: [E40] ID is not 0xE8: 0xF2  [00 00 00 00 00 00 00 00 01 F2]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_expect_ihw_invalidate_tdh_and_next_next() {
        const _VALID_ID: u8 = 0xF0;
        // Boring but very typical TDT, everything is 0 except for packet_done
        let raw_data_tdt = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF1];
        let raw_data_tdt_next = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF2];
        let raw_data_tdt_next_next = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF3];

        let (send, stats_recv_ch) = std::sync::mpsc::channel();
        let mut mock_cfg = MockChecks::new();
        mock_cfg
            .expect_check()
            .times(1)
            .returning(|| Option::Some(crate::util::config::Check::All(Target { system: None })));
        let mut validator = CdpRunningValidator::<RdhCRU<V7>>::default();
        validator.set_config(&mock_cfg);
        validator.stats_send_ch = send;
        let rdh_mem_pos = 0x0; // RDH size is 64 bytes

        validator.set_current_rdh(&CORRECT_RDH_CRU_V7, rdh_mem_pos); // Data format is 2
        validator.check(&raw_data_tdt);
        validator.check(&raw_data_tdt_next);
        validator.check(&raw_data_tdt_next_next);

        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x40: [E30] ID is not 0xE0: 0xF1  [00 00 00 00 00 00 00 00 01 F1]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x4A: [E40] ID is not 0xE8: 0xF2  [00 00 00 00 00 00 00 00 01 F2]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                assert_eq!(
                    msg,
                    "0x4A: [E44] TDH trigger_orbit is not equal to RDH orbit [00 00 00 00 00 00 00 00 01 F2]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
        match stats_recv_ch.recv() {
            Ok(StatType::Error(msg)) => {
                // Data word error
                assert_eq!(
                    msg,
                    "0x54: [E70] ID is invalid: 0xF3 [00 00 00 00 00 00 00 00 01 F3]"
                );
                println!("{msg}");
            }
            _ => unreachable!(),
        }
    }
}
