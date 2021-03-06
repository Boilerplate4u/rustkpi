use kernel;
use kernel::ptr::Unique;

use kernel::sys::raw::*;
use kernel::prelude::v1::*;

use sys::e1000::*;
use sys::e1000_consts::*;

use iflib::*;
use hw::*;
use consts::*;
use bridge::*;
use adapter::*;
use e1000_mac;
use e1000_osdep::*;
use e1000_regs::*;
use e1000_phy;
use e1000_nvm;

pub const fn fn_phy_reg(page: u32, reg: u32) -> u32 {
    (page << PHY_PAGE_SHIFT) | (reg & MAX_PHY_REG_ADDRESS)
}

/* SMBus Control Phy Register */
const CV_SMB_CTRL: u32 = fn_phy_reg(769, 23);
/* SMBus Address Phy Register */
const HV_SMB_ADDR: u32 = fn_phy_reg(768, 26);
/* PHY Power Management Control */
const HV_PM_CTRL: u32 = fn_phy_reg(770, 17);
/* LED Configuration */
const HV_LED_CONFIG: u32 = fn_phy_reg(768, 30);
/* OEM Bits Phy Register */
const HV_OEM_BITS: u32 = fn_phy_reg(768, 25);
/* I218 Ultra Low Power Configuration 1 Register */
const I218_ULP_CONFIG1: u32 = fn_phy_reg(779, 16);
const BM_PORT_GEN_CFG: u32 = fn_phy_reg(BM_PORT_CTRL_PAGE, 17);
const HV_SCC_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 16); /* Single Collision */
const HV_SCC_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 17);
const HV_ECOL_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 18); /* Excessive Coll. */
const HV_ECOL_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 19);
const HV_MCC_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 20); /* Multiple Collision */
const HV_MCC_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 21);
const HV_LATECOL_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 23); /* Late Collision */
const HV_LATECOL_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 24);
const HV_COLC_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 25);
const HV_COLC_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 26);
const HV_DC_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 27);
const HV_DC_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 28);
const HV_TNCRS_UPPER: u32 = fn_phy_reg(HV_STATS_PAGE, 29);
const HV_TNCRS_LOWER: u32 = fn_phy_reg(HV_STATS_PAGE, 30);
const I217_PLL_CLOCK_GATE_REG: u32 = fn_phy_reg(772, 28);
/* KMRN FIFO Control and Status */
const HV_KMRN_FIFO_CTRLSTA: u32 = fn_phy_reg(770, 16);
/* PHY Low Power Idle Control */
const I82579_LPI_CTRL: u32 = fn_phy_reg(772, 20);
/* Inband Control */
const I217_INBAND_CTRL: u32 = fn_phy_reg(770, 18);

///  e1000_init_function_pointers_ich8lan - Initialize ICH8 function pointers
///  @hw: pointer to the HW structure
///
///  Initialize family-specific function pointers for PHY, MAC, and NVM.
pub fn init_function_pointers(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    adapter.hw.mac.ops.init_params = Some(init_mac_params_ich8lan);
    adapter.hw.nvm.ops.init_params = Some(init_nvm_params_ich8lan);

    match adapter.hw.mac.mac_type {
        MacType::Mac_pchlan
        | MacType::Mac_pch2lan
        | MacType::Mac_pch_lpt
        | MacType::Mac_pch_spt
        | MacType::Mac_pch_cnp => {
            adapter.hw.phy.ops.init_params = Some(init_phy_params_pchlan);
        }
        _ => {
            incomplete!();
            return Err("Unsupported hardware".to_string());
        }
    }

    Ok(())
}

/// e1000_phy_is_accessible_pchlan - Check if able to access PHY registers
/// @hw: pointer to the HW structure
///
/// Test access to the PHY registers by reading the PHY ID registers.  If
/// the PHY ID is already known (e.g. resume path) compare it with known ID,
/// otherwise assume the read PHY ID is correct if it is valid.
///
/// Assumes the sw/fw/hw semaphore is already acquired.
pub fn phy_is_accessible_pchlan(adapter: &mut Adapter) -> Result<bool, String> {
    e1000_println!();

    let mut phy_reg: u16 = 0;
    let mut phy_id: u32 = 0;
    let mut retry_count: u16 = 0;
    let mut mac_reg: u32 = 0;

    for i in 0..2 {
        let res = adapter.phy_read_reg_locked(PHY_ID1, &mut phy_reg);
        if res.is_err() || phy_reg == 0xFFFF {
            eprintln!("(IGNORE) {:?}", res.unwrap_err());
            continue;
        }
        phy_id = (phy_reg as u32) << 16;

        let res = adapter.phy_read_reg_locked(PHY_ID2, &mut phy_reg);
        if res.is_err() || phy_reg == 0xFFFF {
            eprintln!("(IGNORE) {:?}", res.unwrap_err());
            phy_id = 0;
            continue;
        }
        phy_id |= (phy_reg as u32) & PHY_REVISION_MASK;
        break;
    }

    'out: loop {
        let mut res = Ok(());
        if adapter.hw.phy.id != 0 {
            if adapter.hw.phy.id == phy_id {
                break 'out;
            }
        } else if phy_id != 0 {
            adapter.hw.phy.id = phy_id;
            adapter.hw.phy.revision = phy_reg as u32 & !PHY_REVISION_MASK;
            break 'out;
        }
        if adapter.hw.mac.mac_type < MacType::Mac_pch_lpt {
            try!(adapter.phy_release());
            res = set_mdio_slow_mode_hv(adapter);
            if res.is_ok() {
                res = e1000_phy::get_phy_id(adapter);
            }
            try!(adapter.phy_acquire());
        }
        if res.is_err() {
            eprintln!("(IGNORE) {:?}", res.unwrap_err());
            return Ok(false);
        }
        break 'out;
    }
    if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
        if !btst!(adapter.read_register(E1000_FWSM), E1000_ICH_FWSM_FW_VALID) {
            try!(
                adapter
                    .phy_read_reg_locked(CV_SMB_CTRL, &mut phy_reg)
                    .or_else::<String, _>(|e| {
                        eprintln!("(IGNORE) {:?}", e);
                        Ok(())
                    })
            );
            phy_reg &= !(CV_SMB_CTRL_FORCE_SMBUS as u16);
            try!(
                adapter
                    .phy_write_reg_locked(CV_SMB_CTRL, phy_reg)
                    .or_else::<String, _>(|e| {
                        eprintln!("(IGNORE) {:?}", e);
                        Ok(())
                    })
            );
            adapter.clear_register_bit(E1000_CTRL_EXT, E1000_CTRL_EXT_FORCE_SMBUS);
        }
    }
    Ok(true)
}

/// e1000_toggle_lanphypc_pch_lpt - toggle the LANPHYPC pin value
/// @hw: pointer to the HW structure
///
/// Toggling the LANPHYPC pin value fully power-cycles the PHY and is
/// used to reset the PHY to a quiescent state when necessary.
pub fn toggle_lanphypc_pch_lpt(adapter: &mut Adapter) {
    e1000_println!();

    let mut mac_reg: u32;

    /* Set Phy Config Counter to 50msec */
    mac_reg = adapter.read_register(E1000_FEXTNVM3);
    mac_reg &= !E1000_FEXTNVM3_PHY_CFG_COUNTER_MASK;
    mac_reg |= E1000_FEXTNVM3_PHY_CFG_COUNTER_50MSEC;
    adapter.write_register(E1000_FEXTNVM3, mac_reg);

    /* Toggle LANPHYPC Value bit */
    mac_reg = adapter.read_register(E1000_CTRL);
    mac_reg |= E1000_CTRL_LANPHYPC_OVERRIDE;
    mac_reg &= !E1000_CTRL_LANPHYPC_VALUE;
    adapter.write_register(E1000_CTRL, mac_reg);
    adapter.write_flush();

    do_msec_delay(1);
    mac_reg &= !E1000_CTRL_LANPHYPC_OVERRIDE;
    adapter.write_register(E1000_CTRL, mac_reg);
    adapter.write_flush();

    if adapter.hw.mac.mac_type < MacType::Mac_pch_lpt {
        do_msec_delay(50);
    } else {
        let mut count = 20;
        loop {
            do_msec_delay(5);
            if btst!(adapter.read_register(E1000_CTRL_EXT), E1000_CTRL_EXT_LPCD) || count == 0 {
                break;
            }
            count -= 1;
        }
        do_msec_delay(30);
    }
}

/// e1000_init_phy_workarounds_pchlan - PHY initialization workarounds
/// @hw: pointer to the HW structure
///
/// Workarounds/flow necessary for PHY initialization during driver load
/// and resume paths.
pub fn init_phy_workarounds_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut ret_val = 0;

    fn out(adapter: &mut Adapter, fwsm: u32) {
        if adapter.is_mac(MacType::Mac_pch2lan) && !btst!(fwsm, E1000_ICH_FWSM_FW_VALID) {
            do_msec_delay(10);
            gate_hw_phy_config_ich8lan(adapter, false);
        }
    }

    let mut mac_reg: u32;
    let mut fwsm: u32 = adapter.read_register(E1000_FWSM);

    /* Gate automatic PHY configuration by hardware on managed and
     * non-managed 82579 and newer adapters.
     */
    gate_hw_phy_config_ich8lan(adapter, true);

    /* It is not possible to be certain of the current state of ULP
     * so forcibly disable it.
     */
    unsafe {
        adapter.hw.dev_spec.ich8lan.ulp_state = UlpState::Unknown;
    }

    if let Err(e) = disable_ulp_lpt_lp(adapter, true) {
        eprintln!("(IGNORE) {:?}", e);
    }

    try!(adapter.phy_acquire());

    /* The MAC-PHY interconnect may be in SMBus mode.  If the PHY is
     * inaccessible and resetting the PHY is not blocked, toggle the
     * LANPHYPC Value bit to force the interconnect to PCIe mode.
     */

    'switch: loop {
        let macs1 = [
            MacType::Mac_pch_lpt,
            MacType::Mac_pch_spt,
            MacType::Mac_pch_cnp,
        ];
        let macs2 = [MacType::Mac_pch2lan];
        let macs3 = [MacType::Mac_pchlan];

        if adapter.is_macs(&macs1) {
            match phy_is_accessible_pchlan(adapter) {
                Ok(true) => break 'switch,
                Ok(false) => (),
                Err(e) => eprintln!("{:?}", e),
            }
            adapter.set_register_bit(E1000_CTRL_EXT, E1000_CTRL_EXT_FORCE_SMBUS);
            do_msec_delay(50);
        }
        if adapter.is_macs(&macs1) || adapter.is_macs(&macs2) {
            match phy_is_accessible_pchlan(adapter) {
                Ok(true) => break 'switch,
                Ok(false) => (),
                Err(e) => eprintln!("(NON FATAL) {:?}", e),
            }
        }
        if adapter.is_macs(&macs1) || adapter.is_macs(&macs2) || adapter.is_macs(&macs3) {
            if adapter.is_mac(MacType::Mac_pchlan) && btst!(fwsm, E1000_ICH_FWSM_FW_VALID) {
                break 'switch;
            }
            match try!(adapter.check_reset_block()) {
                true => {
                    e1000_println!("Required LANPHYPC toggle blocked by ME");
                    ret_val = E1000_ERR_PHY;
                    break 'switch;
                }
                false => (),
            }
            toggle_lanphypc_pch_lpt(adapter);
            if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
                match phy_is_accessible_pchlan(adapter) {
                    Ok(true) => break 'switch,
                    Ok(false) => (),
                    Err(e) => eprintln!("(IGNORE) {:?}", e),
                }
                adapter.clear_register_bit(E1000_CTRL_EXT, E1000_CTRL_EXT_FORCE_SMBUS);
                match phy_is_accessible_pchlan(adapter) {
                    Ok(true) => break 'switch,
                    Ok(false) => (),
                    Err(e) => eprintln!("(IGNORE) {:?}", e),
                }
                ret_val = E1000_ERR_PHY;
            }
        }
        break 'switch;
    }
    try!(adapter.phy_release());

    if ret_val == 0 {
        try!(match adapter.check_reset_block() {
            Ok(true) => {
                eprintln!("Reset blocked by ME");
                out(adapter, fwsm);
                Err("Reset blocked by ME".to_string())
            }
            Ok(false) => Ok(()),
            Err(e) => Err(e),
        });

        /* Reset the PHY before any access to it.  Doing so, ensures
         * that the PHY is in a known good state before we read/write
         * PHY registers.  The generic reset is sufficient here,
         * because we haven't determined the PHY type yet.
         */
        let res = e1000_phy::phy_hw_reset_generic(adapter);
        if res.is_err() {
            out(adapter, fwsm);
            return res;
        }

        /* On a successful reset, possibly need to wait for the PHY
         * to quiesce to an accessible state before returning control
         * to the calling function.  If the PHY does not quiesce, then
         * return E1000E_BLK_PHY_RESET, as this is the condition that
         * the PHY is in.
         */
        try!(match adapter.check_reset_block() {
            Ok(true) => {
                eprintln!("ME blocked access to PHY after reset");
                out(adapter, fwsm);
                Err("Reset blocked by ME".to_string())
            }
            Ok(false) => Ok(()),
            Err(e) => Err(e),
        });
    }

    /* Ungate automatic PHY configuration on non-managed 82579 */
    out(adapter, fwsm);
    if ret_val == 0 {
        Ok(())
    } else {
        Err("PHY error".to_string())
    }
}

/// e1000_init_phy_params_pchlan - Initialize PHY function pointers
/// @hw: pointer to the HW structure
///
/// Initialize family-specific PHY parameters and function pointers.
pub fn init_phy_params_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    {
        let phy: &mut PhyInfo = &mut adapter.hw.phy;

        phy.addr = 1;
        phy.reset_delay_us = 100;

        phy.ops.acquire = Some(acquire_swflag_ich8lan);
        phy.ops.check_reset_block = Some(check_reset_block_ich8lan);
        phy.ops.get_cfg_done = Some(get_cfg_done_ich8lan);
        phy.ops.set_page = Some(e1000_phy::set_page_igp);
        phy.ops.read_reg = Some(e1000_phy::read_phy_reg_hv);
        phy.ops.read_reg_locked = Some(e1000_phy::read_phy_reg_hv_locked);
        phy.ops.read_reg_page = Some(e1000_phy::read_phy_reg_page_hv);
        phy.ops.release = Some(release_swflag_ich8lan);
        phy.ops.reset = Some(phy_hw_reset_ich8lan);
        phy.ops.set_d0_lplu_state = Some(set_lplu_state_pchlan);
        phy.ops.set_d3_lplu_state = Some(set_lplu_state_pchlan);
        phy.ops.write_reg = Some(e1000_phy::write_phy_reg_hv);
        phy.ops.write_reg_locked = Some(e1000_phy::write_phy_reg_hv_locked);
        phy.ops.write_reg_page = Some(e1000_phy::write_phy_reg_page_hv);
        phy.ops.power_up = Some(e1000_phy::power_up_phy_copper);
        phy.ops.power_down = Some(power_down_phy_copper_ich8lan);

        phy.autoneg_mask = AUTONEG_ADVERTISE_SPEED_DEFAULT as u16;
        phy.id = PhyType::Type_unknown as u32;
    }

    try!(init_phy_workarounds_pchlan(adapter));
    e1000_println!("init_phy_workarounds_pchlan() done");

    'switch: loop {
        if adapter.hw.phy.id == PhyType::Type_unknown as u32 {
            try!(e1000_phy::get_phy_id(adapter));
            e1000_println!("get_phy_id() done");
            if adapter.hw.phy.id != 0 && adapter.hw.phy.id != PHY_REVISION_MASK {
                break 'switch;
            }
            let macs = [
                MacType::Mac_pch2lan,
                MacType::Mac_pch_lpt,
                MacType::Mac_pch_spt,
                MacType::Mac_pch_cnp,
            ];
            if adapter.is_macs(&macs) {
                try!(set_mdio_slow_mode_hv(adapter));
                e1000_println!("set_mdio_slow_mode_hv() done");
                try!(e1000_phy::get_phy_id(adapter));
                e1000_println!("get_phy_id() done");
            }
        }
        break 'switch;
    }
    adapter.hw.phy.phy_type = e1000_phy::get_phy_type_from_id(adapter.hw.phy.id);
    e1000_println!("get_phy_type_from_id() done. {:?}", adapter.hw.phy.phy_type);

    match adapter.hw.phy.phy_type {
        PhyType::Type_82577 | PhyType::Type_82579 | PhyType::Type_i217 => {
            adapter.hw.phy.ops.check_polarity = Some(e1000_phy::check_polarity_82577);
            adapter.hw.phy.ops.force_speed_duplex = Some(e1000_phy::phy_force_speed_duplex_82577);
            adapter.hw.phy.ops.get_cable_length = Some(e1000_phy::get_cable_length_82577);
            adapter.hw.phy.ops.get_info = Some(e1000_phy::get_phy_info_82577);
            adapter.hw.phy.ops.commit = Some(e1000_phy::phy_sw_reset_generic);
        }
        PhyType::Type_82578 => {
            adapter.hw.phy.ops.check_polarity = Some(e1000_phy::check_polarity_m88);
            adapter.hw.phy.ops.force_speed_duplex = Some(e1000_phy::phy_force_speed_duplex_m88);
            adapter.hw.phy.ops.get_cable_length = Some(e1000_phy::get_cable_length_m88);
            adapter.hw.phy.ops.get_info = Some(e1000_phy::get_phy_info_m88);
        }
        _ => return Err("Unknown phy type".to_string()),
    }
    Ok(())
}

/// e1000_init_phy_params_ich8lan - Initialize PHY function pointers
/// @hw: pointer to the HW structure
///
/// Initialize family-specific PHY parameters and function pointers.
pub fn init_phy_params_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    unsupported!();
    Err("This hardware is not supported".to_string())
}

/// e1000_init_nvm_params_ich8lan - Initialize NVM function pointers
/// @hw: pointer to the HW structure
///
/// Initialize family-specific NVM parameters and function
/// pointers.
pub fn init_nvm_params_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mac_type = adapter.hw.mac.mac_type;
    adapter.hw.nvm.nvm_type = NvmType::FlashSw;

    if mac_type >= MacType::Mac_pch_spt {
        adapter.hw.nvm.flash_base_addr = 0;
        let nvm_size =
            (((adapter.read_register(E1000_STRAP) >> 1) & 0x1F) + 1) * NVM_SIZE_MULTIPLIER;
        adapter.hw.nvm.flash_bank_size = nvm_size / 2;
        adapter.hw.nvm.flash_bank_size /= kernel::mem::size_of::<u16>() as u32;
        adapter.hw.flash_address =
            unsafe { adapter.hw.hw_addr.offset(E1000_FLASH_BASE_ADDR as isize) };
        e1000_println!("Setting adapter.hw.flash_address from hw_addr");
    } else {
        e1000_println!("Using mapped adapter.hw.flash_address");
        if adapter.hw.flash_address == kernel::ptr::null_mut() {
            return Err("Flash registers not mapped".to_string());
        }

        let gfpreg = adapter.read_flash_register(ICH_FLASH_GFPREG);

        /* sector_X_addr is a "sector"-aligned address (4096 bytes)
         *  Add 1 to sector_end_addr since this sector is included in
         *  the overall size.
         */
        let sector_base_addr = gfpreg & FLASH_GFPREG_BASE_MASK;
        let sector_end_addr = ((gfpreg >> 16) & FLASH_GFPREG_BASE_MASK) + 1;

        /* flash_base_addr is byte-aligned */
        adapter.hw.nvm.flash_base_addr = sector_base_addr << FLASH_SECTOR_ADDR_SHIFT;

        /* find total size of the NVM, then cut in half since the total
         *  size represents two separate NVM banks.
         */
        let mut s = (sector_end_addr - sector_base_addr) << FLASH_SECTOR_ADDR_SHIFT;
        s /= 2;
        s /= kernel::mem::size_of::<u16>() as u32;
        adapter.hw.nvm.flash_bank_size = s;
        e1000_println!("Got bank size: {} (words)", s);
    }
    adapter.hw.nvm.word_size = E1000_SHADOW_RAM_WORDS as u16;

    /* Clear shadow ram */
    unsafe {
        let srs: &mut [ShadowRam; 2048] = &mut adapter.hw.dev_spec.ich8lan.shadow_ram;
        for sr in &mut srs.iter_mut() {
            sr.modified = false;
            sr.value = 0xFFFF;
        }
    }

    /* Function Pointers */
    adapter.hw.nvm.ops.acquire = Some(acquire_nvm_ich8lan);
    adapter.hw.nvm.ops.release = Some(release_nvm_ich8lan);
    if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
        adapter.hw.nvm.ops.read = Some(read_nvm_spt);
        adapter.hw.nvm.ops.update = Some(update_nvm_checksum_spt);
    } else {
        adapter.hw.nvm.ops.read = Some(read_nvm_ich8lan);
        adapter.hw.nvm.ops.update = Some(update_nvm_checksum_ich8lan);
    }
    adapter.hw.nvm.ops.valid_led_default = Some(valid_led_default_ich8lan);
    adapter.hw.nvm.ops.validate = Some(validate_nvm_checksum_ich8lan);
    adapter.hw.nvm.ops.write = Some(write_nvm_ich8lan);

    Ok(())
}

/// e1000_init_mac_params_ich8lan - Initialize MAC function pointers
/// @hw: pointer to the HW structure
///
/// Initialize family-specific MAC parameters and function
/// pointers.
pub fn init_mac_params_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    /* Set media type function pointer */
    adapter.hw.phy.media_type = MediaType::Copper;
    {
        let mac: &mut MacInfo = &mut adapter.hw.mac;

        /* Set mta register count */
        mac.mta_reg_count = 32;

        /* Set rar entry count */
        mac.rar_entry_count = E1000_ICH_RAR_ENTRIES as u16;

        if mac.mac_type == MacType::Mac_ich8lan {
            mac.rar_entry_count -= 1;
        }

        /* Set if part includes ASF firmware */
        mac.asf_firmware_present = true;

        /* FWSM register */
        mac.has_fwsm = true;

        /* ARC subsystem not supported */
        mac.arc_subsystem_valid = false;

        /* Adaptive IFS supported */
        mac.adaptive_ifs = true;

        /* Function pointers */

        /* bus type/speed/width */
        mac.ops.get_bus_info = Some(get_bus_info_ich8lan);

        /* function id */
        mac.ops.set_lan_id = Some(e1000_mac::set_lan_id_single_port);

        /* reset */
        mac.ops.reset_hw = Some(reset_hw_ich8lan);

        /* hw initialization */
        mac.ops.init_hw = Some(init_hw_ich8lan);

        /* link setup */
        mac.ops.setup_link = Some(setup_link_ich8lan);

        /* physical interface setup */
        mac.ops.setup_physical_interface = Some(setup_copper_link_ich8lan);

        /* check for link */
        mac.ops.check_for_link = Some(check_for_copper_link_ich8lan);

        /* link info */
        mac.ops.get_link_up_info = Some(get_link_up_info_ich8lan);

        /* multicast address update */
        mac.ops.update_mc_addr_list = Some(e1000_mac::update_mc_addr_list_generic);

        /* clear hardware counters */
        mac.ops.clear_hw_cntrs = Some(clear_hw_cntrs_ich8lan);

        /* LED and other operations */
        let ich_macs = [
            MacType::Mac_ich8lan,
            MacType::Mac_ich9lan,
            MacType::Mac_ich10lan,
        ];
        let pch_macs = [
            MacType::Mac_pch_cnp,
            MacType::Mac_pch_lpt,
            MacType::Mac_pch_spt,
        ];
        'switch: loop {
            if ich_macs.contains(&mac.mac_type) {
                /* check management mode */
                mac.ops.check_mng_mode = Some(check_mng_mode_ich8lan);
                /* ID LED init */
                mac.ops.id_led_init = Some(e1000_mac::id_led_init_generic);
                /* blink LED */
                mac.ops.blink_led = Some(e1000_mac::blink_led_generic);
                /* setup LED */
                mac.ops.setup_led = Some(e1000_mac::setup_led_generic);
                /* cleanup LED */
                mac.ops.cleanup_led = Some(cleanup_led_ich8lan);
                /* turn on/off LED */
                mac.ops.led_on = Some(led_on_ich8lan);
                mac.ops.led_off = Some(led_off_ich8lan);
                break 'switch;
            }
            if mac.mac_type == MacType::Mac_pch2lan {
                mac.rar_entry_count = E1000_PCH2_RAR_ENTRIES as u16;
                mac.ops.rar_set = Some(rar_set_pch2lan);
            }
            if mac.mac_type == MacType::Mac_pch2lan || pch_macs.contains(&mac.mac_type) {
                mac.ops.update_mc_addr_list = Some(update_mc_addr_list_pch2lan);
            }
            if mac.mac_type == MacType::Mac_pch2lan || pch_macs.contains(&mac.mac_type)
                || mac.mac_type == MacType::Mac_pchlan
            {
                /* check management mode */
                mac.ops.check_mng_mode = Some(check_mng_mode_pchlan);
                /* ID LED init */
                mac.ops.id_led_init = Some(id_led_init_pchlan);
                /* setup LED */
                mac.ops.setup_led = Some(setup_led_pchlan);
                /* cleanup LED */
                mac.ops.cleanup_led = Some(cleanup_led_pchlan);
                /* turn on/off LED */
                mac.ops.led_on = Some(led_on_pchlan);
                mac.ops.led_off = Some(led_off_pchlan);
                break 'switch;
            }
            break 'switch;
        }

        if mac.mac_type >= MacType::Mac_pch_lpt {
            mac.rar_entry_count = E1000_PCH_LPT_RAR_ENTRIES as u16;
            mac.ops.rar_set = Some(rar_set_pch_lpt);
            mac.ops.setup_physical_interface = Some(setup_copper_link_pch_lpt);
            mac.ops.set_obff_timer = Some(set_obff_timer_pch_lpt);
        }
    } // end adapter.hw.mac mutable borrow

    /* Enable PCS Lock-loss workaround for ICH8 */
    if adapter.hw.mac.mac_type == MacType::Mac_ich8lan {
        set_kmrn_lock_loss_workaround_ich8lan(adapter, true);
    }
    Ok(())
}

/// __e1000_access_emi_reg_locked - Read/write EMI register
/// @hw: pointer to the HW structure
/// @addr: EMI address to program
/// @data: pointer to value to read/write from/to the EMI address
/// @read: boolean flag to indicate read or write
///
/// This helper function assumes the SW/FW/HW Semaphore is already acquired.
pub fn access_emi_reg_locked(
    adapter: &mut Adapter,
    address: u16,
    data: &mut u16,
    read: bool,
) -> AdResult {
    e1000_println!();

    try!(adapter.phy_write_reg_locked(I82579_EMI_ADDR, address));

    if read {
        adapter.phy_read_reg_locked(I82579_EMI_DATA, data)
    } else {
        adapter.phy_write_reg_locked(I82579_EMI_DATA, *data)
    }
}

/// e1000_read_emi_reg_locked - Read Extended Management Interface register
/// @hw: pointer to the HW structure
/// @addr: EMI address to program
/// @data: value to be read from the EMI address
///
/// Assumes the SW/FW/HW Semaphore is already acquired.
pub fn read_emi_reg_locked(adapter: &mut Adapter, addr: u16, data: &mut u16) -> AdResult {
    e1000_println!();
    access_emi_reg_locked(adapter, addr, data, true)
}

/// e1000_write_emi_reg_locked - Write Extended Management Interface register
/// @hw: pointer to the HW structure
/// @addr: EMI address to program
/// @data: value to be written to the EMI address
///
/// Assumes the SW/FW/HW Semaphore is already acquired.
pub fn write_emi_reg_locked(adapter: &mut Adapter, addr: u16, mut data: u16) -> AdResult {
    e1000_println!();
    access_emi_reg_locked(adapter, addr, &mut data, false)
}

/// e1000_set_eee_pchlan - Enable/disable EEE support
/// @hw: pointer to the HW structure
///
/// Enable/disable EEE based on setting in dev_spec structure, the duplex of
/// the link and the EEE capabilities of the link partner.  The LPI Control
/// register bits will remain set only if/when link is up.
///
/// EEE LPI must not be asserted earlier than one second after link is up.
/// On 82579, EEE LPI should not be enabled until such time otherwise there
/// can be link issues with some switches.  Other devices can have EEE LPI
/// enabled immediately upon link up since they have a timer in hardware which
/// prevents LPI from being asserted too early.
pub fn set_eee_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut lpa: u16 = 0;
    let mut pcs_status: u16 = 0;
    let mut adv: u16 = 0;
    let mut adv_addr: u16 = 0;
    let mut lpi_ctrl: u16 = 0;
    let mut data: u16 = 0;

    match adapter.hw.phy.phy_type {
        PhyType::Type_82579 => {
            unsupported!();
            incomplete_return!();
        }
        PhyType::Type_i217 => {
            lpa = I217_EEE_LP_ABILITY as u16;
            pcs_status = I217_EEE_PCS_STATUS as u16;
            adv_addr = I217_EEE_ADVERTISEMENT as u16;
        }
        _ => return Ok(()),
    }

    try!(adapter.phy_acquire());
    try!(adapter.phy_read_reg_locked(I82579_LPI_CTRL, &mut lpi_ctrl));

    /* Clear bits that enable EEE in various speeds */
    lpi_ctrl &= !(I82579_LPI_CTRL_ENABLE_MASK as u16);

    /* Enable EEE if not disabled by user */
    let eee_disable: bool = unsafe { adapter.hw.dev_spec.ich8lan.eee_disable };
    if !eee_disable {
        /* Save off link partner's EEE ability */
        try!(
            read_emi_reg_locked(adapter, lpa, &mut unsafe {
                adapter.hw.dev_spec.ich8lan.eee_lp_ability
            }).or_else(|e| {
                try!(adapter.phy_release());
                Err(e)
            })
        );

        /* Read EEE advertisement */
        try!(
            read_emi_reg_locked(adapter, adv_addr, &mut adv).or_else(|e| {
                try!(adapter.phy_release());
                Err(e)
            },)
        );

        /* Enable EEE only for speeds in which the link partner is
         * EEE capable and for which we advertise EEE.
         */
        if (adv & unsafe { adapter.hw.dev_spec.ich8lan.eee_lp_ability }
            & I82579_EEE_1000_SUPPORTED as u16) != 0
        {
            lpi_ctrl |= I82579_LPI_CTRL_1000_ENABLE as u16;
        }

        if (adv & unsafe { adapter.hw.dev_spec.ich8lan.eee_lp_ability }
            & I82579_EEE_100_SUPPORTED as u16) != 0
        {
            try!(adapter.phy_read_reg_locked(PHY_LP_ABILITY, &mut data));
            if btst!(data, NWAY_LPAR_100TX_FD_CAPS) {
                lpi_ctrl |= I82579_LPI_CTRL_100_ENABLE as u16;
            } else {
                unsafe {
                    adapter.hw.dev_spec.ich8lan.eee_lp_ability &=
                        !(I82579_EEE_100_SUPPORTED as u16);
                }
            }
        }
    }

    if adapter.hw.phy.phy_type == PhyType::Type_82579 {
        unsupported!();
        incomplete_return!();
    }

    /* R/Clr IEEE MMD 3.1 bits 11:10 - Tx/Rx LPI Received */
    try!(
        read_emi_reg_locked(adapter, pcs_status, &mut data).or_else(|e| {
            try!(adapter.phy_release());
            Err(e)
        })
    );

    let res = adapter.phy_write_reg_locked(I82579_LPI_CTRL, lpi_ctrl);

    adapter.phy_release().and(res)
}

/// e1000_k1_workaround_lpt_lp - K1 workaround on Lynxpoint-LP
/// @hw:   pointer to the HW structure
/// @link: link up bool flag
///
/// When K1 is enabled for 1Gbps, the MAC can miss 2 DMA completion indications
/// preventing further DMA write requests.  Workaround the issue by disabling
/// the de-assertion of the clock request when in 1Gpbs mode.
/// Also, set appropriate Tx re-transmission timeouts for 10 and 100Half link
/// speeds in order to avoid Tx hangs.
pub fn k1_workaround_lpt_lp(adapter: &mut Adapter, link: bool) -> AdResult {
    e1000_println!();

    let mut fextnvm6: u32 = adapter.read_register(E1000_FEXTNVM6);
    let status: u32 = adapter.read_register(E1000_STATUS);
    let mut reg: u16 = 0;

    if link && btst!(status, E1000_STATUS_SPEED_1000) {
        try!(adapter.phy_acquire());

        try!(
            e1000_phy::read_kmrn_reg_locked(adapter, E1000_KMRNCTRLSTA_K1_CONFIG, &mut reg)
                .or_else(|e| {
                    try!(adapter.phy_release());
                    Err(e)
                })
        );

        try!(
            e1000_phy::write_kmrn_reg_locked(
                adapter,
                E1000_KMRNCTRLSTA_K1_CONFIG,
                reg & !(E1000_KMRNCTRLSTA_K1_ENABLE as u16),
            ).or_else(|e| {
                try!(adapter.phy_release());
                Err(e)
            })
        );

        do_usec_delay(10);

        adapter.write_register(E1000_FEXTNVM6, fextnvm6 | E1000_FEXTNVM6_REQ_PLL_CLK);

        let res = e1000_phy::write_kmrn_reg_locked(adapter, E1000_KMRNCTRLSTA_K1_CONFIG, reg);

        try!(adapter.phy_release().and(res));
    } else {
        /* clear FEXTNVM6 bit 8 on link down or 10/100 */
        fextnvm6 &= !E1000_FEXTNVM6_REQ_PLL_CLK;

        if adapter.hw.phy.revision > 5 || !link
            || (btst!(status, E1000_STATUS_SPEED_100) && btst!(status, E1000_STATUS_FD))
        {
            adapter.write_register(E1000_FEXTNVM6, fextnvm6);
            return Ok(());
        }
        try!(adapter.phy_read_reg(I217_INBAND_CTRL, &mut reg));

        /* Clear link status transmit timeout */
        reg &= !I217_INBAND_CTRL_LINK_STAT_TX_TIMEOUT_MASK as u16;

        if btst!(status, E1000_STATUS_SPEED_100) {
            /* Set inband Tx timeout to 5x10us for 100Half */
            reg |= 5 << I217_INBAND_CTRL_LINK_STAT_TX_TIMEOUT_SHIFT;
            /* Do not extend the K1 entry latency for 100Half */
            fextnvm6 &= !E1000_FEXTNVM6_ENABLE_K1_ENTRY_CONDITION;
        } else {
            /* Set inband Tx timeout to 50x10us for 10Full/Half */
            reg |= 50 << I217_INBAND_CTRL_LINK_STAT_TX_TIMEOUT_SHIFT;
            /* Extend the K1 entry latency for 10 Mbps */
            fextnvm6 |= E1000_FEXTNVM6_ENABLE_K1_ENTRY_CONDITION;
        }

        adapter.phy_write_reg(I217_INBAND_CTRL, reg);
        adapter.write_register(E1000_FEXTNVM6, fextnvm6);
    }
    Ok(())
}

pub fn ltr2ns(ltr: u16) -> u64 {
    e1000_println!();

    /* Determine the latency in nsec based on the LTR value & scale */
    let value: u32 = ltr as u32 & E1000_LTRV_VALUE_MASK;
    let scale: u32 = (ltr as u32 & E1000_LTRV_SCALE_MASK) >> E1000_LTRV_SCALE_SHIFT;

    (value * (1 << (scale * E1000_LTRV_SCALE_FACTOR))) as u64
}

/// e1000_platform_pm_pch_lpt - Set platform power management values
/// @hw: pointer to the HW structure
/// @link: bool indicating link status
///
/// Set the Latency Tolerance Reporting (LTR) values for the "PCIe-like"
/// GbE MAC in the Lynx Point PCH based on Rx buffer size and link speed
/// when link is up (which must not exceed the maximum latency supported
/// by the platform), otherwise specify there is no LTR requirement.
/// Unlike TRUE-PCIe devices which set the LTR maximum snoop/no-snoop
/// latencies in the LTR Extended Capability Structure in the PCIe Extended
/// Capability register set, on this device LTR is set by writing the
/// equivalent snoop/no-snoop latencies in the LTRV register in the MAC and
/// set the SEND bit to send an Intel On-chip System Fabric sideband (IOSF-SB)
/// message to the PMC.
///
/// Use the LTR value to calculate the Optimized Buffer Flush/Fill (OBFF)
/// high-water mark.
pub fn platform_pm_pch_lpt(adapter: &mut Adapter, link: bool) -> AdResult {
    e1000_println!();
    let l: u32 = link as u32;
    let mut reg: u32 = l << (E1000_LTRV_REQ_SHIFT + E1000_LTRV_NOSNOOP_SHIFT)
        | (l << E1000_LTRV_REQ_SHIFT) | E1000_LTRV_SEND;
    let mut lat_enc: u16 = 0;
    let mut obff_hwm: i32 = 0;

    if link {
        let mut speed: u16 = 0;
        let mut duplex: u16 = 0;
        let mut scale: u16 = 0;
        let mut max_snoop: u16;
        let mut max_nosnoop: u16;
        let mut max_ltr_enc: u16;
        let mut lat_ns: i64;
        let mut value: i64;
        let mut rxa: u32;

        if adapter.hw.mac.max_frame_size == 0 {
            return Err("Max frame size is not set".to_string());
        }

        try!(
            adapter
                .hw
                .mac
                .ops
                .get_link_up_info
                .ok_or("No function".to_string())
                .and_then(
                    |f| f(adapter, &mut speed, &mut duplex).and_then(|()| if speed == 0 {
                        Err("Speed not set".to_string())
                    } else {
                        Ok(())
                    })
                )
        );

        /* Rx Packet Buffer Allocation size (KB) */
        rxa = adapter.read_register(E1000_PBA) & E1000_PBA_RXA_MASK;

        /* Determine the maximum latency tolerated by the device.
         *
         * Per the PCIe spec, the tolerated latencies are encoded as
         * a 3-bit encoded scale (only 0-5 are valid) multiplied by
         * a 10-bit value (0-1023) to provide a range from 1 ns to
         * 2^25*(2^10-1) ns.  The scale is encoded as 0=2^0ns,
         * 1=2^5ns, 2=2^10ns,...5=2^25ns.
         */
        lat_ns = (rxa as i64) * 1024 - (2 * (adapter.hw.mac.max_frame_size as i64)) * 8 * 1000;
        if lat_ns < 0 {
            lat_ns = 0;
        } else {
            lat_ns /= speed as i64;
        }
        value = lat_ns;

        while value > E1000_LTRV_VALUE_MASK as i64 {
            scale += 1;
            value = divide_round_up!(value, (1 << 5));
        }
        if scale > E1000_LTRV_SCALE_MAX as u16 {
            return Err(format!("Invalid LTR latency scale {}", scale));
        }
        lat_enc = ((scale << E1000_LTRV_SCALE_SHIFT) | value as u16);

        /* Determine the maximum latency tolerated by the platform */
        max_snoop = adapter.dev.pci_read_config(E1000_PCI_LTR_CAP_LPT, 2) as u16;
        max_nosnoop = adapter.dev.pci_read_config(E1000_PCI_LTR_CAP_LPT + 2, 2) as u16;
        use core::cmp;
        max_ltr_enc = cmp::max(max_snoop, max_nosnoop);

        if lat_enc > max_ltr_enc {
            lat_enc = max_ltr_enc;
            lat_ns = ltr2ns(max_ltr_enc) as i64;
        }
        if lat_ns != 0 {
            lat_ns *= speed as i64 * 1000;
            lat_ns /= 8;
            lat_ns /= 1_000_000_000;
            obff_hwm = (rxa as i64 - lat_ns) as i32;
        }

        if obff_hwm < 0 || obff_hwm > E1000_SVT_OFF_HWM_MASK as i32 {
            return Err(format!("Invalid high water mark {}", obff_hwm));
        }
    }

    /* Set Snoop and No-Snoop latencies the same */
    reg |= lat_enc as u32 | ((lat_enc as u32) << E1000_LTRV_NOSNOOP_SHIFT);
    adapter.write_register(E1000_LTRV, reg);

    /* Set OBFF high water mark */
    reg = adapter.read_register(E1000_SVT) & !E1000_SVT_OFF_HWM_MASK;
    reg |= obff_hwm as u32;
    adapter.write_register(E1000_SVT, reg);

    /* Enable OBFF */
    reg = adapter.read_register(E1000_SVCR);
    reg |= E1000_SVCR_OFF_EN;

    /* Always unblock interrupts to the CPU even when the system is
     * in OBFF mode. This ensures that small round-robin traffic
     * (like ping) does not get dropped or experience long latency.
     */
    reg |= E1000_SVCR_OFF_MASKINT;
    adapter.write_register(E1000_SVCR, reg);
    Ok(())
}

/// e1000_set_obff_timer_pch_lpt - Update Optimized Buffer Flush/Fill timer
/// @hw: pointer to the HW structure
/// @itr: interrupt throttling rate
///
/// Configure OBFF with the updated interrupt rate.
pub fn set_obff_timer_pch_lpt(adapter: &mut Adapter, itr: u32) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_enable_ulp_lpt_lp - configure Ultra Low Power mode for LynxPoint-LP
/// @hw: pointer to the HW structure
/// @to_sx: boolean indicating a system power state transition to Sx
///
/// When link is down, configure ULP mode to significantly reduce the power
/// to the PHY.  If on a Manageability Engine (ME) enabled system, tell the
/// ME firmware to start the ULP configuration.  If not on an ME enabled
/// system, configure the ULP mode by software.
pub fn enable_ulp_lpt_lp(adapter: &mut Adapter, to_sx: bool) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_disable_ulp_lpt_lp - unconfigure Ultra Low Power mode for LynxPoint-LP
/// @hw: pointer to the HW structure
/// @force: boolean indicating whether or not to force disabling ULP
///
/// Un-configure ULP mode when link is up, the system is transitioned from
/// Sx or the driver is unloaded.  If on a Manageability Engine (ME) enabled
/// system, poll for an indication from ME that ULP has been un-configured.
/// If not on an ME enabled system, un-configure the ULP mode by software.
///
/// During nominal operation, this function is called when link is acquired
/// to disable ULP mode (force=FALSE); otherwise, for example when unloading
/// the driver or during Sx->S0 transitions, this is called with force=TRUE
/// to forcibly disable ULP.
pub fn disable_ulp_lpt_lp(adapter: &mut Adapter, force: bool) -> AdResult {
    e1000_println!();

    fn _release(adapter: &mut Adapter, force: bool) -> AdResult {
        try!(adapter.phy_release());
        if force {
            try!(adapter.phy_reset());
            do_msec_delay(50);
        }
        Ok(())
    };

    fn set_state(adapter: &mut Adapter) {
        unsafe {
            adapter.hw.dev_spec.ich8lan.ulp_state = UlpState::Off;
        }
    }

    let skip_devids = [
        E1000_DEV_ID_PCH_LPT_I217_LM,
        E1000_DEV_ID_PCH_LPT_I217_V,
        E1000_DEV_ID_PCH_I218_LM2,
        E1000_DEV_ID_PCH_I218_V2,
    ];

    let state = unsafe { adapter.hw.dev_spec.ich8lan.ulp_state };
    if adapter.hw.mac.mac_type < MacType::Mac_pch_lpt || state == UlpState::Off
        || skip_devids.contains(&(adapter.hw.device_id as u32))
    {
        e1000_println!("Skipping ulp disable for this device");
        return Ok(());
    }

    let mut mac_reg: u32;
    let mut phy_reg: u16 = 0;
    let mut i: usize;

    if btst!(adapter.read_register(E1000_FWSM), E1000_ICH_FWSM_FW_VALID) {
        e1000_println!("Got E1000_ICH_FWSM_FW_VALID");

        if force {
            /* Request ME un-configure ULP mode in the PHY */
            mac_reg = adapter.read_register(E1000_H2ME);
            mac_reg &= !E1000_H2ME_ULP;
            mac_reg |= E1000_H2ME_ENFORCE_SETTINGS;
            adapter.write_register(E1000_H2ME, mac_reg);
        }

        /* Poll up to 300msec for ME to clear ULP_CFG_DONE. */
        i = 0;
        while btst!(adapter.read_register(E1000_FWSM), E1000_FWSM_ULP_CFG_DONE) {
            if i == 30 {
                return Err("ULP configure".to_string());
            }
            i += 1;
            do_msec_delay(10);
        }
        e1000_println!("ULP_CONFIG_DONE cleared after {} ms", i * 10);

        if force {
            adapter.clear_register_bit(E1000_H2ME, E1000_H2ME_ENFORCE_SETTINGS);
        } else {
            /* Clear H2ME.ULP after ME ULP configuration */
            adapter.clear_register_bit(E1000_H2ME, E1000_H2ME_ULP);
        }
        set_state(adapter);
        return Ok(());
    } else {
        e1000_println!("E1000_ICH_FWSM_FW_VALID false - skip");
    }

    try!(adapter.phy_acquire());

    if force {
    	/* Toggle LANPHYPC Value bit */
        toggle_lanphypc_pch_lpt(adapter);
        e1000_println!("toggle_lanphypc_pch_lpt() done");
    }

    /* Unforce SMBus mode in PHY */
    if let Err(e) = e1000_phy::read_phy_reg_hv_locked(adapter, CV_SMB_CTRL, &mut phy_reg) {
        eprintln!(e);
        e1000_println!("Force to smbus mode so we can access the PHY");
        adapter.set_register_bit(E1000_CTRL_EXT, E1000_CTRL_EXT_FORCE_SMBUS);
        do_msec_delay(50);
        if let Err(e) = e1000_phy::read_phy_reg_hv_locked(adapter, CV_SMB_CTRL, &mut phy_reg) {
            eprintln!(e);
            return _release(adapter, force);
        };
    }

    e1000_println!("Un-force to smbus mode");
    phy_reg &= !CV_SMB_CTRL_FORCE_SMBUS as u16;
    let res = e1000_phy::write_phy_reg_hv_locked(adapter, CV_SMB_CTRL, phy_reg);
    if let Err(e) = res {
        eprintln!(e);
    }

    /* Unforce SMBus mode in MAC */
    adapter.clear_register_bit(E1000_CTRL_EXT, E1000_CTRL_EXT_FORCE_SMBUS);

    /* When ULP mode was previously entered, K1 was disabled by the
     * hardware.  Re-Enable K1 in the PHY when exiting ULP.
     */
    if let Err(e) = e1000_phy::read_phy_reg_hv_locked(adapter, HV_PM_CTRL, &mut phy_reg) {
        eprintln!(e);
        return _release(adapter, force);
    }

    phy_reg |= HV_PM_CTRL_K1_ENABLE;
    if let Err(e) = e1000_phy::write_phy_reg_hv_locked(adapter, HV_PM_CTRL, phy_reg) {
        eprintln!(e);
    }

    /* Clear ULP enabled configuration */
    if let Err(e) = e1000_phy::read_phy_reg_hv_locked(adapter, I218_ULP_CONFIG1, &mut phy_reg) {
        eprintln!(e);
        return _release(adapter, force);
    }

    phy_reg &= !(I218_ULP_CONFIG1_IND | I218_ULP_CONFIG1_STICKY_ULP
        | I218_ULP_CONFIG1_RESET_TO_SMBUS | I218_ULP_CONFIG1_WOL_HOST
        | I218_ULP_CONFIG1_INBAND_EXIT | I218_ULP_CONFIG1_EN_ULP_LANPHYPC
        | I218_ULP_CONFIG1_DIS_CLR_STICKY_ON_PERST
        | I218_ULP_CONFIG1_DISABLE_SMB_PERST);

    if let Err(e) = e1000_phy::write_phy_reg_hv_locked(adapter, I218_ULP_CONFIG1, phy_reg) {
        eprintln!(e);
    }

    /* Commit ULP changes by starting auto ULP configuration */
    phy_reg |= I218_ULP_CONFIG1_START;
    let res = e1000_phy::write_phy_reg_hv_locked(adapter, I218_ULP_CONFIG1, phy_reg);
    if let Err(e) = res {
        eprintln!(e);
    }

    /* Clear Disable SMBus Release on PERST# in MAC */
    adapter.clear_register_bit(E1000_FEXTNVM7, E1000_FEXTNVM7_DISABLE_SMB_PERST);

    try!(_release(adapter, force));
    set_state(adapter);
    Ok(())
}

/// e1000_check_for_copper_link_ich8lan - Check for link (Copper)
/// @hw: pointer to the HW structure
///
/// Checks to see of the link status of the hardware has changed.  If a
/// change in link status has been detected, then we read the PHY registers
/// to get the current speed/duplex if link exists.
pub fn check_for_copper_link_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut tipg_reg: u32 = 0;
    let mut emi_addr: u16;
    let mut emi_val: u16 = 0;
    let mut link: bool = false;
    let mut phy_reg: u16 = 0;

    // TODO: use adapter. functions
    let read_reg = try!(adapter.hw.phy.ops.read_reg.ok_or("No function".to_string()));
    let read_reg_locked = try!(
        adapter
            .hw
            .phy
            .ops
            .read_reg_locked
            .ok_or("No function".to_string(),)
    );
    let write_reg = try!(
        adapter
            .hw
            .phy
            .ops
            .write_reg
            .ok_or("No function".to_string(),)
    );
    let write_reg_locked = try!(
        adapter
            .hw
            .phy
            .ops
            .write_reg_locked
            .ok_or("No function".to_string(),)
    );

    /* We only want to go out to the PHY registers to see if Auto-Neg
     * has completed and/or if our link status has changed.  The
     * get_link_status flag is set upon receiving a Link Status
     * Change or Rx Sequence Error interrupt.
     */
    if !adapter.hw.mac.get_link_status {
        return Ok(());
    }

    /* First we want to see if the MII Status Register reports
     * link.  If so, then we want to get the current speed/duplex
     * of the PHY.
     */
    try!(e1000_phy::has_link_generic(adapter, 1, 0, &mut link));

    if adapter.hw.mac.mac_type == MacType::Mac_pchlan {
        unsupported!();
        incomplete_return!();
    }

    /* When connected at 10Mbps half-duplex, some parts are excessively
     * aggressive resulting in many collisions. To avoid this, increase
     * the IPG and reduce Rx latency in the PHY.
     */
    if adapter.hw.mac.mac_type >= MacType::Mac_pch2lan && link {
        let mut speed: u16 = 0;
        let mut duplex: u16 = 0;

        e1000_mac::get_speed_and_duplex_copper_generic(adapter, &mut speed, &mut duplex);

        tipg_reg = adapter.read_register(E1000_TIPG);
        tipg_reg &= !E1000_TIPG_IPGT_MASK;

        if duplex == HALF_DUPLEX && speed == SPEED_10 {
            tipg_reg |= 0xFF;
        	/* Reduce Rx latency in analog PHY */
            emi_val = 0;
        } else if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt && duplex == FULL_DUPLEX
            && speed != SPEED_1000
        {
            tipg_reg |= 0x0C;
            emi_val = 1;
        } else {
        	/* Roll back the default values */
            tipg_reg |= 0x08;
            emi_val = 1;
        }

        adapter.write_register(E1000_TIPG, tipg_reg);

        try!(adapter.phy_acquire());

        if adapter.hw.mac.mac_type == MacType::Mac_pch2lan {
            emi_addr = I82579_RX_CONFIG as u16;
        } else {
            emi_addr = I217_RX_CONFIG as u16;
        }

        let write_emi_result = write_emi_reg_locked(adapter, emi_addr, emi_val);

        if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
            let mut phy_reg: u16 = 0;

            if let Err(e) = read_reg_locked(adapter, I217_PLL_CLOCK_GATE_REG, &mut phy_reg) {
                eprintln!("(IGNORE) {:?}", e);
            }

            phy_reg &= !I217_PLL_CLOCK_GATE_MASK as u16;
            if speed == SPEED_100 || speed == SPEED_10 {
                phy_reg |= 0x3E8;
            } else {
                phy_reg |= 0xFA;
            }

            if let Err(e) = write_reg_locked(adapter, I217_PLL_CLOCK_GATE_REG, phy_reg) {
                eprintln!("(IGNORE) {:?}", e);
            }

            if speed == SPEED_1000 {
                if let Err(e) = read_reg_locked(adapter, HV_PM_CTRL, &mut phy_reg) {
                    eprintln!("(IGNORE) {:?}", e);
                }
                phy_reg |= HV_PM_CTRL_K1_CLK_REQ;
                if let Err(e) = write_reg_locked(adapter, HV_PM_CTRL, phy_reg) {
                    eprintln!("(IGNORE) {:?}", e);
                }
            }
        }
        try!(adapter.phy_release());

        if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
            let mut data: u16 = 0;
            let mut ptr_gap: u16 = 0;

            if speed == SPEED_1000 {
                try!(adapter.phy_acquire());
                try!(
                    read_reg_locked(adapter, fn_phy_reg(776, 20), &mut data).or_else(|e| {
                        try!(adapter.phy_release());
                        Err(e)
                    })
                );
                ptr_gap = (data & (0x3FF << 2)) >> 2;
                if ptr_gap < 0x18 {
                    data &= !(0x3FF << 2);
                    data |= (0x18 << 2);
                    try!(
                        write_reg_locked(adapter, fn_phy_reg(776, 20), data).or_else(|e| {
                            try!(adapter.phy_release());
                            Err(e)
                        })
                    );
                } else {
                    try!(adapter.phy_release());
                }
            } else {
                try!(adapter.phy_acquire());
                let res = write_reg_locked(adapter, fn_phy_reg(776, 20), 0xC023);
                try!(adapter.phy_release());
                try!(res);
            }
        }
    }
    /* I217 Packet Loss issue:
     * ensure that FEXTNVM4 Beacon Duration is set correctly
     * on power up.
     * Set the Beacon Duration for I217 to 8 usec
     */
    if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
        let mut mac_reg: u32;
        mac_reg = adapter.read_register(E1000_FEXTNVM4);
        mac_reg &= !E1000_FEXTNVM4_BEACON_DURATION_MASK;
        mac_reg |= E1000_FEXTNVM4_BEACON_DURATION_8USEC;
        adapter.write_register(E1000_FEXTNVM4, mac_reg);
    }

    /* Work-around I218 hang issue */
    if [
        E1000_DEV_ID_PCH_LPTLP_I218_LM,
        E1000_DEV_ID_PCH_LPTLP_I218_V,
        E1000_DEV_ID_PCH_I218_LM3,
        E1000_DEV_ID_PCH_I218_V3,
    ].contains(&(adapter.hw.device_id as u32))
    {
        try!(k1_workaround_lpt_lp(adapter, link));
    }

    if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
        /* Set platform power management values for
         * Latency Tolerance Reporting (LTR)
         * Optimized Buffer Flush/Fill (OBFF)
         */
        try!(platform_pm_pch_lpt(adapter, link));
    }

    /* Clear link partner's EEE ability */
    unsafe {
        adapter.hw.dev_spec.ich8lan.eee_lp_ability = 0;
    }

    if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
        let mut fextnvm6: u32 = adapter.read_register(E1000_FEXTNVM6);

        if adapter.hw.mac.mac_type == MacType::Mac_pch_spt {
            let pcieanacfg: u32 = adapter.read_register(E1000_PCIEANACFG);
            if btst!(pcieanacfg, E1000_FEXTNVM6_K1_OFF_ENABLE) {
                fextnvm6 |= E1000_FEXTNVM6_K1_OFF_ENABLE;
            } else {
                fextnvm6 &= !E1000_FEXTNVM6_K1_OFF_ENABLE;
            }
        }
        if unsafe { adapter.hw.dev_spec.ich8lan.disable_k1_off } == true {
            fextnvm6 &= !E1000_FEXTNVM6_K1_OFF_ENABLE;
        }
        adapter.write_register(E1000_FEXTNVM6, fextnvm6);
    }

    if !link {
        return Ok(());
    }

    adapter.hw.mac.get_link_status = false;

    if adapter.is_mac(MacType::Mac_pch2lan) {
        unsupported!();
        incomplete_return!();
    }

    if adapter.is_macs(&[MacType::Mac_pch2lan, MacType::Mac_pchlan]) {
        if adapter.hw.phy.phy_type == PhyType::Type_82578 {
            unsupported!();
            incomplete_return!();
        }

        /* Workaround for PCHx parts in half-duplex:
         * Set the number of preambles removed from the packet
         * when it is passed from the PHY to the MAC to prevent
         * the MAC from misinterpreting the packet type.
         */
        read_reg(adapter, HV_KMRN_FIFO_CTRLSTA, &mut phy_reg);

        phy_reg &= !HV_KMRN_FIFO_CTRLSTA_PREAMBLE_MASK;

        if adapter.read_register(E1000_STATUS) & E1000_STATUS_FD != E1000_STATUS_FD {
            phy_reg |= (1 << HV_KMRN_FIFO_CTRLSTA_PREAMBLE_SHIFT);
        }
        if let Err(e) = write_reg(adapter, HV_KMRN_FIFO_CTRLSTA, phy_reg) {
            eprintln!("(IGNORE) {:?}", e);
        }
    }

    /* Check if there was DownShift, must be checked
     * immediately after link-up
     */
    if let Err(e) = e1000_phy::check_downshift_generic(adapter) {
        eprintln!("(IGNORE) {:?}", e);
    }

    /* Enable/Disable EEE after link up */
    if adapter.hw.phy.phy_type > PhyType::Type_82579 {
        try!(set_eee_pchlan(adapter));
    }

    /* If we are forcing speed/duplex, then we simply return since
     * we have already determined whether we have link or not.
     */
    if !adapter.hw.mac.autoneg {
        return Err("Auto-Neg is not set".to_string());
    }

    /* Auto-Neg is enabled.  Auto Speed Detection takes care
     * of MAC speed/duplex configuration.  So we only need to
     * configure Collision Distance in the MAC.
     */
    try!(
        adapter
            .hw
            .mac
            .ops
            .config_collision_dist
            .ok_or("No function".to_string())
            .and_then(|f| {
                f(adapter);
                Ok(())
            })
    );

    /* Configure Flow Control now that Auto-Neg has completed.
     * First, we need to restore the desired flow control
     * settings because we may have had to re-autoneg with a
     * different link partner.
     */
    e1000_mac::config_fc_after_link_up_generic(adapter)
}

/// e1000_acquire_nvm_ich8lan - Acquire NVM mutex
/// @hw: pointer to the HW structure
///
/// Acquires the mutex for performing NVM operations.
pub fn acquire_nvm_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_verbose_println!();
    assert_ctx_lock_held!(adapter);
    Ok(())
}

/// e1000_release_nvm_ich8lan - Release NVM mutex
/// @hw: pointer to the HW structure
///
/// Releases the mutex used while performing NVM operations.
pub fn release_nvm_ich8lan(adapter: &mut Adapter) {
    e1000_verbose_println!();
    assert_ctx_lock_held!(adapter);
}

/// e1000_acquire_swflag_ich8lan - Acquire software control flag
/// @hw: pointer to the HW structure
///
/// Acquires the software control flag for performing PHY and select
/// MAC CSR accesses.
pub fn acquire_swflag_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_verbose_println!();

    assert_ctx_lock_held!(adapter);

    let mut extcnf_ctrl = 0;
    let mut timeout = PHY_CFG_TIMEOUT;
    while timeout > 0 {
        extcnf_ctrl = adapter.read_register(E1000_EXTCNF_CTRL);
        if !btst!(extcnf_ctrl, E1000_EXTCNF_CTRL_SWFLAG) {
            break;
        }
        do_msec_delay(1);
        timeout -= 1;
    }
    if timeout == 0 {
        return Err("SW has already locked the resource".to_string());
    }

    extcnf_ctrl |= E1000_EXTCNF_CTRL_SWFLAG;
    adapter.write_register(E1000_EXTCNF_CTRL, extcnf_ctrl);

    timeout = SW_FLAG_TIMEOUT;
    while timeout > 0 {
        extcnf_ctrl = adapter.read_register(E1000_EXTCNF_CTRL);
        if btst!(extcnf_ctrl, E1000_EXTCNF_CTRL_SWFLAG) {
            break;
        }
        do_msec_delay(1);
        timeout -= 1;
    }
    if timeout == 0 {
        extcnf_ctrl &= !E1000_EXTCNF_CTRL_SWFLAG;
        adapter.write_register(E1000_EXTCNF_CTRL, extcnf_ctrl);
        return Err("Failed to acquire the semaphore, FW or HW has it.".to_string());
    }
    Ok(())
}

/// e1000_release_swflag_ich8lan - Release software control flag
/// @hw: pointer to the HW structure
///
/// Releases the software control flag for performing PHY and select
/// MAC CSR accesses.
pub fn release_swflag_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_verbose_println!();

    let mut extcnf_ctrl = adapter.read_register(E1000_EXTCNF_CTRL);

    if btst!(extcnf_ctrl, E1000_EXTCNF_CTRL_SWFLAG) {
        extcnf_ctrl &= !E1000_EXTCNF_CTRL_SWFLAG;
        adapter.write_register(E1000_EXTCNF_CTRL, extcnf_ctrl);
    } else {
        eprintln!("Semaphore unexpectedly released by sw/fw/hw");
    }
    assert_ctx_lock_held!(adapter);

    Ok(())
}

/// e1000_check_mng_mode_ich8lan - Checks management mode
/// @hw: pointer to the HW structure
///
/// This checks if the adapter has any manageability enabled.
/// This is a function pointer entry point only called by read/write
/// routines for the PHY and NVM parts.
pub fn check_mng_mode_ich8lan(adapter: &mut Adapter) -> bool {
    e1000_println!();
    incomplete!();
    false
}

/// e1000_check_mng_mode_pchlan - Checks management mode
/// @hw: pointer to the HW structure
///
/// This checks if the adapter has iAMT enabled.
/// This is a function pointer entry point only called by read/write
/// routines for the PHY and NVM parts.
pub fn check_mng_mode_pchlan(adapter: &mut Adapter) -> bool {
    e1000_println!();
    incomplete!();
    false
}

/// e1000_rar_set_pch2lan - Set receive address register
/// @hw: pointer to the HW structure
/// @addr: pointer to the receive address
/// @index: receive address array register
///
/// Sets the receive address array register at index to the address passed
/// in by addr.  For 82579, RAR[0] is the base address register that is to
/// contain the MAC address but RAR[1-6] are reserved for manageability (ME).
/// Use SHRA[0-3] in place of those reserved for ME.
pub fn rar_set_pch2lan(adapter: &mut Adapter, addr: &[u8], index: usize) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_rar_set_pch_lpt - Set receive address registers
/// @hw: pointer to the HW structure
/// @addr: pointer to the receive address
/// @index: receive address array register
///
/// Sets the receive address register array at index to the address passed
/// in by addr. For LPT, RAR[0] is the base address register that is to
/// contain the MAC address. SHRA[0-10] are the shared receive address
/// registers that are shared between the Host and manageability engine (ME).
pub fn rar_set_pch_lpt(adapter: &mut Adapter, addr: &[u8], index: usize) -> AdResult {
    e1000_println!();
    let mut wlock_mac: u32;

    /* HW expects these in little endian so we reverse the byte order
     * from network order (big endian) to little endian
     */
    let (rar_low, mut rar_high): (u32, u32) = {
        // let addr = &adapter.hw.mac.addr;
        let low: u32 = (addr[0] as u32) | (addr[1] as u32) << 8 | (addr[2] as u32) << 16
            | (addr[3] as u32) << 24;
        let high: u32 = (addr[4] as u32) | (addr[5] as u32) << 8;
        (low, high)
    };

    /* If MAC address zero, no need to set the AV bit */
    if rar_low != 0 || rar_high != 0 {
        rar_high |= E1000_RAH_AV;
    }

    if index == 0 {
        do_write_register(adapter, E1000_RAL(index), rar_low);
        do_write_flush(adapter);
        do_write_register(adapter, E1000_RAH(index), rar_high);
        do_write_flush(adapter);
        return Ok(());
    }

    /* The manageability engine (ME) can lock certain SHRAR registers that
     * it is using - those registers are unavailable for use.
     */
    if index < adapter.hw.mac.rar_entry_count as usize {
        wlock_mac = adapter.read_register(E1000_FWSM) & E1000_FWSM_WLOCK_MAC_MASK;
        wlock_mac >>= E1000_FWSM_WLOCK_MAC_SHIFT;
        /* Check if all SHRAR registers are locked */
        if wlock_mac == 1 {
            return Err(format!(
                "Failed to write receive address at index {}",
                index
            ));
        }
        if wlock_mac == 0 || index <= wlock_mac as usize {
            try!(acquire_swflag_ich8lan(adapter));

            adapter.write_register(E1000_SHRAL_PCH_LPT(index - 1), rar_low);
            adapter.write_flush();
            adapter.write_register(E1000_SHRAH_PCH_LPT(index - 1), rar_high);
            adapter.write_flush();

            try!(release_swflag_ich8lan(adapter));

            /* verify the register updates */
            if adapter.read_register(E1000_SHRAL_PCH_LPT(index - 1)) != rar_low
                || adapter.read_register(E1000_SHRAH_PCH_LPT(index - 1)) != rar_high
            {
                return Err(format!(
                    "Failed to write receive address at index {}",
                    index
                ));
            }
        }
    }
    Ok(())
}

/// e1000_update_mc_addr_list_pch2lan - Update Multicast addresses
/// @hw: pointer to the HW structure
/// @mc_addr_list: array of multicast addresses to program
/// @mc_addr_count: number of multicast addresses to program
///
/// Updates entire Multicast Table Array of the PCH2 MAC and PHY.
/// The caller must have a packed mc_addr_list of multicast addresses.
pub fn update_mc_addr_list_pch2lan(adapter: &mut Adapter, mc_addr_count: u32) -> AdResult {
    e1000_println!();

    let mut phy_reg: u16 = 0;

    e1000_mac::update_mc_addr_list_generic(adapter, mc_addr_count);

    try!(adapter.phy_acquire());
    try!(
        e1000_phy::enable_phy_wakeup_reg_access_bm(adapter, &mut phy_reg)
            .or_else(|e| adapter.phy_release().and(Err(e)))
    );

    let write = match adapter.hw.phy.ops.write_reg_page {
        Some(f) => f,
        None => {
            eprintln!("No function");
            return adapter.phy_release();
        }
    };

    for i in 0..adapter.hw.mac.mta_reg_count as usize {
        try!(
            write(
                adapter,
                BM_MTA(i),
                (adapter.hw.mac.mta_shadow[i] as u16) & 0xFFFF,
            ).or_else(|e| adapter.phy_release().and(Err(e)))
        );
        try!(
            write(
                adapter,
                BM_MTA(i) + 1,
                ((adapter.hw.mac.mta_shadow[i] >> 16) as u16) & 0xFFFF,
            ).or_else(|e| adapter.phy_release().and(Err(e)))
        );
    }
    try!(
        e1000_phy::disable_phy_wakeup_reg_access_bm(adapter, &mut phy_reg)
            .or_else(|e| adapter.phy_release().and(Err(e)))
    );

    try!(adapter.phy_release());
    Ok(())
}

/// e1000_check_reset_block_ich8lan - Check if PHY reset is blocked
/// @hw: pointer to the HW structure
///
/// Checks if firmware is blocking the reset of the PHY.
/// This is a function pointer entry point only called by
/// reset routines.
pub fn check_reset_block_ich8lan(adapter: &mut Adapter) -> Result<bool, String> {
    e1000_println!();

    let mut fwsm: u32;
    let mut blocked;
    let mut i = 0;

    loop {
        fwsm = adapter.read_register(E1000_FWSM);
        if !btst!(fwsm, E1000_ICH_FWSM_RSPCIPHY) {
            blocked = true;
            do_msec_delay(10);
        } else {
            blocked = false;
        }
        i += 1;
        if !blocked || i > 30 {
            break;
        }
    }
    Ok(blocked)
}

/// e1000_write_smbus_addr - Write SMBus address to PHY needed during Sx states
/// @hw: pointer to the HW structure
///
/// Assumes semaphore already acquired.
///
pub fn write_smbus_addr(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    let mut phy_data: u16 = 0;
    let mut strap: u32 = adapter.read_register(E1000_STRAP);
    let mut freq: u32 = (strap & E1000_STRAP_SMT_FREQ_MASK) >> E1000_STRAP_SMT_FREQ_SHIFT;

    strap &= E1000_STRAP_SMBUS_ADDRESS_MASK;

    try!(e1000_phy::read_phy_reg_hv_locked(
        adapter,
        HV_SMB_ADDR,
        &mut phy_data,
    ));

    phy_data &= !HV_SMB_ADDR_MASK as u16;
    phy_data |= (strap >> E1000_STRAP_SMBUS_ADDRESS_SHIFT) as u16;
    phy_data |= (HV_SMB_ADDR_PEC_EN | HV_SMB_ADDR_VALID) as u16;

    if adapter.hw.phy.phy_type == PhyType::Type_i217 {
        if freq > 0 {
            freq -= 1;
            phy_data &= !HV_SMB_ADDR_FREQ_MASK as u16;
            phy_data |= ((freq & (1 << 0)) << HV_SMB_ADDR_FREQ_LOW_SHIFT) as u16;
            phy_data |= ((freq & (1 << 1)) << (HV_SMB_ADDR_FREQ_HIGH_SHIFT - 1)) as u16;
        } else {
            e1000_println!("Unsupported SMB frequency in PHY\n");
        }
    }
    e1000_phy::write_phy_reg_hv_locked(adapter, HV_SMB_ADDR, phy_data)
}

/// e1000_sw_lcd_config_ich8lan - SW-based LCD Configuration
/// @hw:   pointer to the HW structure
///
/// SW should configure the LCD from the NVM extended configuration region
/// as a workaround for certain parts.
pub fn sw_lcd_config_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut data: u32;
    let mut cnf_size: u32;
    let mut cnf_base_addr: u32;
    let mut sw_cfg_mask: u32 = 0;
    let mut word_addr: u16;
    let mut reg_data: [u16; 1] = [0];
    let mut reg_addr: [u16; 1] = [0];
    let mut phy_page: u16 = 0;

    /* Initialize the PHY from the NVM on ICH platforms.  This
     *  is needed due to an issue where the NVM configuration is
     *  not properly autoloaded after power transitions.
     *  Therefore, after each PHY reset, we will load the
     *  configuration data out of the NVM manually.
     */

    'mac: loop {
        if adapter.is_mac(MacType::Mac_ich8lan) {
            if adapter.hw.phy.phy_type != PhyType::Type_igp_3 {
                e1000_println!("Returning early based on MAC and PHY type");
                return Ok(());
            }
            if adapter.hw.device_id == E1000_DEV_ID_ICH8_IGP_AMT as u16
                || adapter.hw.device_id == E1000_DEV_ID_ICH8_IGP_C as u16
            {
                sw_cfg_mask = E1000_FEXTNVM_SW_CONFIG;
                e1000_println!("Set sw_cfg_mask to E1000_FEXTNVM_SW_CONFIG");
                break 'mac;
            }
        }
        let pchs = [
            MacType::Mac_pchlan,
            MacType::Mac_pch2lan,
            MacType::Mac_pch_cnp,
            MacType::Mac_pch_lpt,
            MacType::Mac_pch_spt,
        ];
        if adapter.is_mac(MacType::Mac_ich8lan) || adapter.is_macs(&pchs) {
            sw_cfg_mask = E1000_FEXTNVM_SW_CONFIG_ICH8M;
            break 'mac;
        }
        e1000_println!("Returning early from end of mac loop");
        return Ok(());
    }
    try!(adapter.phy_acquire());

    data = adapter.read_register(E1000_FEXTNVM);
    if !btst!(data, sw_cfg_mask) {
        e1000_println!("Returning early sw_cfg_mask match error");
        return adapter.phy_release();
    }

    /* Make sure HW does not configure LCD from PHY
     *  extended configuration before SW configuration
     */
    data = adapter.read_register(E1000_EXTCNF_CTRL);
    if adapter.hw.mac.mac_type < MacType::Mac_pch2lan
        && btst!(data, E1000_EXTCNF_CTRL_LCD_WRITE_ENABLE)
    {
        e1000_println!("Returning early at LCD check");
        return adapter.phy_release();
    }

    cnf_size = adapter.read_register(E1000_EXTCNF_SIZE);
    cnf_size &= E1000_EXTCNF_SIZE_EXT_PCIE_LENGTH_MASK;
    cnf_size >>= E1000_EXTCNF_SIZE_EXT_PCIE_LENGTH_SHIFT;
    if cnf_size == 0 {
        e1000_println!("Returning early because cnf_size = 0");
        return adapter.phy_release();
    }

    cnf_base_addr = data & E1000_EXTCNF_CTRL_EXT_CNF_POINTER_MASK;
    cnf_base_addr >>= E1000_EXTCNF_CTRL_EXT_CNF_POINTER_SHIFT;

    if (adapter.is_mac(MacType::Mac_pchlan) && !btst!(data, E1000_EXTCNF_CTRL_OEM_WRITE_ENABLE))
        || adapter.hw.mac.mac_type > MacType::Mac_pchlan
    {
        let res = write_smbus_addr(adapter);
        if res.is_err() {
            e1000_println!("Returning early because write_smbus_failure");
            return adapter.phy_release().and(res);
        }
        data = adapter.read_register(E1000_LEDCTL);
        let res = e1000_phy::write_phy_reg_hv_locked(adapter, HV_LED_CONFIG, data as u16);
        if res.is_err() {
            e1000_println!("Returning early because write_phy_reg HV_LED_CONFIG error");
            return adapter.phy_release().and(res);
        }
    }
    /* Configure LCD from extended configuration region. */

    /* cnf_base_addr is in DWORD */
    word_addr = (cnf_base_addr << 1) as u16;

    for i in 0..cnf_size as u16 {
        let res = adapter.nvm_read(word_addr + i * 2, 1, &mut reg_data);
        if res.is_err() {
            return adapter.phy_release().and(res);
        }

        // ret_val = hw->nvm.ops.read(hw, (word_addr + i *  2 + 1),
        // 			   1, &reg_addr);
        // if (ret_val)
        //     goto release;

        let res = adapter.nvm_read(word_addr + i * 2 + 1, 1, &mut reg_addr);
        if res.is_err() {
            return adapter.phy_release().and(res);
        }

        /* Save off the PHY page for future writes. */
        if reg_addr[0] == IGP01E1000_PHY_PAGE_SELECT as u16 {
            phy_page = reg_data[0];
            continue;
        }
        reg_addr[0] &= PHY_REG_MASK as u16;
        reg_addr[0] |= phy_page;

        let res = adapter.phy_write_reg_locked(reg_addr[0] as u32, reg_data[0]);
        if res.is_err() {
            e1000_println!("Returning early because phy_write_reg_locked error");
            return adapter.phy_release().and(res);
        }
    }

    adapter.phy_release()
}

/// e1000_k1_gig_workaround_hv - K1 Si workaround
/// @hw:   pointer to the HW structure
/// @link: link up bool flag
///
/// If K1 is enabled for 1Gbps, the MAC might stall when transitioning
/// from a lower speed.  This workaround disables K1 whenever link is at 1Gig
/// If link is down, the function will restore the default K1 setting located
/// in the NVM.
pub fn k1_gig_workaround_hv(adapter: &mut Adapter, link: bool) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_configure_k1_ich8lan - Configure K1 power state
/// @hw: pointer to the HW structure
/// @enable: K1 state to configure
///
/// Configure the K1 power state based on the provided parameter.
/// Assumes semaphore already acquired.
///
/// Success returns 0, Failure returns -E1000_ERR_PHY (-2)
pub fn configure_k1_ich8lan(adapter: &mut Adapter, k1_enable: bool) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_oem_bits_config_ich8lan - SW-based LCD Configuration
/// @hw:       pointer to the HW structure
/// @d0_state: boolean if entering d0 or d3 device state
///
/// SW will configure Gbe Disable and LPLU based on the NVM. The four bits are
/// collectively called OEM bits.  The OEM Write Enable bit and SW Config bit
/// in NVM determines whether HW should configure LPLU and Gbe Disable.
pub fn oem_bits_config_ich8lan(adapter: &mut Adapter, d0_state: bool) -> AdResult {
    e1000_println!();

    let mut mac_reg: u32 = 0;
    let mut oem_reg: u16 = 0;

    if adapter.hw.mac.mac_type < MacType::Mac_pchlan {
        return Ok(());
    }

    try!(adapter.phy_acquire());

    if adapter.is_mac(MacType::Mac_pchlan) {
        mac_reg = adapter.read_register(E1000_EXTCNF_CTRL);
        if btst!(mac_reg, E1000_EXTCNF_CTRL_OEM_WRITE_ENABLE) {
            return adapter.phy_release();
        }
    }

    mac_reg = adapter.read_register(E1000_FEXTNVM);
    if !btst!(mac_reg, E1000_FEXTNVM_SW_CONFIG_ICH8M) {
        return adapter.phy_release();
    }

    mac_reg = adapter.read_register(E1000_PHY_CTRL);

    let res = adapter.phy_read_reg_locked(HV_OEM_BITS, &mut oem_reg);
    if res.is_err() {
        return adapter.phy_release().and(res);
    }
    oem_reg &= !(HV_OEM_BITS_GBE_DIS | HV_OEM_BITS_LPLU);

    if d0_state {
        if btst!(mac_reg, E1000_PHY_CTRL_GBE_DISABLE) {
            oem_reg |= HV_OEM_BITS_GBE_DIS;
        }
        if btst!(mac_reg, E1000_PHY_CTRL_D0A_LPLU) {
            oem_reg |= HV_OEM_BITS_LPLU;
        }
    } else {
        if btst!(
            mac_reg,
            E1000_PHY_CTRL_GBE_DISABLE | E1000_PHY_CTRL_NOND0A_GBE_DISABLE
        ) {
            oem_reg |= HV_OEM_BITS_GBE_DIS;
        }
        if btst!(
            mac_reg,
            E1000_PHY_CTRL_D0A_LPLU | E1000_PHY_CTRL_NOND0A_LPLU
        ) {
            oem_reg |= HV_OEM_BITS_LPLU;
        }
    }

    /* Set Restart auto-neg to activate the bits */
    if d0_state || !adapter.is_mac(MacType::Mac_pchlan) {
        match adapter.check_reset_block() {
            Ok(true) => (),
            Ok(false) => oem_reg |= HV_OEM_BITS_RESTART_AN,
            Err(e) => return adapter.phy_release().and(Err(e)),
        }
    }

    let res = adapter.phy_write_reg_locked(HV_OEM_BITS, oem_reg);
    adapter.phy_release().and(res)
}

/// e1000_set_mdio_slow_mode_hv - Set slow MDIO access mode
/// @hw:   pointer to the HW structure
pub fn set_mdio_slow_mode_hv(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_hv_phy_workarounds_ich8lan - A series of Phy workarounds to be
/// done after every PHY reset.
pub fn hv_phy_workarounds_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_copy_rx_addrs_to_phy_ich8lan - Copy Rx addresses from MAC to PHY
/// @hw:   pointer to the HW structure
pub fn copy_rx_addrs_to_phy_ich8lan(adapter: &mut Adapter) {
    e1000_println!();
    incomplete!();
}

pub fn calc_rx_da_crc(mac: &[u8]) -> u32 {
    e1000_println!();
    incomplete!();
    0
}

/// e1000_lv_jumbo_workaround_ich8lan - required for jumbo frame operation
/// with 82579 PHY
/// @hw: pointer to the HW structure
/// @enable: flag to enable/disable workaround when enabling/disabling jumbos
pub fn lv_jumbo_workaround(adapter: &mut Adapter, enable: bool) -> AdResult {
    e1000_println!("enable = {}", enable);

    let mut phy_reg: u16 = 0;
    let mut data: u16 = 0;
    let mut mac_reg: u32 = 0;

    if adapter.hw.mac.mac_type < MacType::Mac_pch2lan {
        return Ok(());
    }

    /* disable Rx path while enabling/disabling workaround */
    try!(adapter.phy_read_reg(fn_phy_reg(769, 20), &mut phy_reg));
    try!(adapter.phy_write_reg(fn_phy_reg(769, 20), phy_reg | (1 << 14),));

    if enable {
        /* Write Rx addresses (rar_entry_count for RAL/H, and
         * SHRAL/H) and initial CRC values to the MAC
         */

        /* Write Rx addresses to the PHY */
        copy_rx_addrs_to_phy_ich8lan(adapter);

        /* Enable jumbo frame workaround in the MAC */
        mac_reg = adapter.read_register(E1000_FFLT_DBG);
        mac_reg &= !(1 << 14);
        mac_reg |= (7 << 15);
        adapter.write_register(E1000_FFLT_DBG, mac_reg);

        adapter.set_register_bit(E1000_RCTL, E1000_RCTL_SECRC);

        try!(e1000_phy::read_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_CTRL_OFFSET,
            &mut data,
        ));

        try!(e1000_phy::write_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_CTRL_OFFSET,
            data | (1 << 0),
        ));

        try!(e1000_phy::read_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_HD_CTRL,
            &mut data,
        ));
        data &= !(0xF << 8);
        data |= 0xB << 8;
        try!(e1000_phy::write_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_HD_CTRL,
            data,
        ));

        /* Enable jumbo frame workaround in the PHY */
        try!(adapter.phy_read_reg(fn_phy_reg(769, 23), &mut data));
        data &= !(0x7F << 5);
        data |= 0x37 << 5;
        try!(adapter.phy_write_reg(fn_phy_reg(769, 23), data));

        try!(adapter.phy_read_reg(fn_phy_reg(769, 16), &mut data));
        data &= !(1 << 13);
        try!(adapter.phy_write_reg(fn_phy_reg(769, 16), data));

        try!(adapter.phy_read_reg(fn_phy_reg(776, 20), &mut data));
        data &= !(0x3FF << 2);
        data |= E1000_TX_PTR_GAP << 2;
        try!(adapter.phy_write_reg(fn_phy_reg(776, 20), data));

        try!(adapter.phy_write_reg(fn_phy_reg(776, 23), 0xF100));

        try!(adapter.phy_read_reg(HV_PM_CTRL, &mut data));
        try!(adapter.phy_write_reg(HV_PM_CTRL, data | (1 << 10)));

        incomplete_return!();
    } else {
        /* Write MAC register values back to h/w defaults */
        adapter.clear_register_bit(E1000_FFLT_DBG, 0xF << 14);

        adapter.clear_register_bit(E1000_RCTL, E1000_RCTL_SECRC);

        try!(e1000_phy::read_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_CTRL_OFFSET,
            &mut data,
        ));
        try!(e1000_phy::write_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_CTRL_OFFSET,
            data & !(1 << 0),
        ));

        try!(e1000_phy::read_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_HD_CTRL,
            &mut data,
        ));
        data &= !(0xF << 8);
        data |= 0xB << 8;
        try!(e1000_phy::write_kmrn_reg_generic(
            adapter,
            E1000_KMRNCTRLSTA_HD_CTRL,
            data,
        ));

        /* Write PHY register values back to h/w defaults */
        try!(adapter.phy_read_reg(fn_phy_reg(769, 23), &mut data));
        data &= !(0x7F << 5);
        try!(adapter.phy_write_reg(fn_phy_reg(769, 23), data));

        try!(adapter.phy_read_reg(fn_phy_reg(769, 16), &mut data));
        data |= 1 << 13;
        try!(adapter.phy_write_reg(fn_phy_reg(769, 16), data));

        try!(adapter.phy_read_reg(fn_phy_reg(776, 20), &mut data));
        data &= !(0x3FF << 2);
        data |= 0x8 << 2;
        try!(adapter.phy_write_reg(fn_phy_reg(776, 20), data));

        try!(adapter.phy_write_reg(fn_phy_reg(776, 23), 0x7E00));

        try!(adapter.phy_read_reg(HV_PM_CTRL, &mut data));
        try!(adapter.phy_write_reg(HV_PM_CTRL, data & !(1 << 10)));
    }

    /* re-enable Rx path after enabling/disabling workaround */
    adapter.phy_write_reg(fn_phy_reg(769, 20), phy_reg & !(1 << 14))
}

/// e1000_lv_phy_workarounds_ich8lan - A series of Phy workarounds to be
/// done after every PHY reset.
pub fn lv_phy_workarounds_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    if !adapter.is_mac(MacType::Mac_pch2lan) {
        return Ok(());
    }

    /* Set MDIO slow mode before any other MDIO access */
    try!(set_mdio_slow_mode_hv(adapter));

    let mut res = Ok(());
    try!(adapter.phy_acquire());

    if write_emi_reg_locked(adapter, I82579_MSE_THRESHOLD as u16, 0x0034).is_ok() {
        res = write_emi_reg_locked(adapter, I82579_MSE_LINK_DOWN as u16, 0x0005);
    } else {
        eprintln!("write_emi_reg_locked");
    }

    try!(adapter.phy_release());

    res
}

/// e1000_k1_gig_workaround_lv - K1 Si workaround
/// @hw:   pointer to the HW structure
///
/// Workaround to set the K1 beacon duration for 82579 parts in 10Mbps
/// Disable K1 for 1000 and 100 speeds
pub fn k1_workaround_lv(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_gate_hw_phy_config_ich8lan - disable PHY config via hardware
/// @hw:   pointer to the HW structure
/// @gate: boolean set to TRUE to gate, FALSE to ungate
///
/// Gate/ungate the automatic PHY configuration via hardware; perform
/// the configuration via software instead.
pub fn gate_hw_phy_config_ich8lan(adapter: &mut Adapter, gate: bool) {
    e1000_println!();

    if adapter.hw.mac.mac_type < MacType::Mac_pch2lan {
        return;
    }

    if gate {
        adapter.set_register_bit(E1000_EXTCNF_CTRL, E1000_EXTCNF_CTRL_GATE_PHY_CFG);
    } else {
        adapter.clear_register_bit(E1000_EXTCNF_CTRL, E1000_EXTCNF_CTRL_GATE_PHY_CFG);
    }
}

/// e1000_lan_init_done_ich8lan - Check for PHY config completion
/// @hw: pointer to the HW structure
///
/// Check the appropriate indication the MAC has finished configuring the
/// PHY after a software reset.
pub fn lan_init_done_ich8lan(adapter: &mut Adapter) {
    e1000_println!();

    let mut data: u32;
    let mut count: u32 = E1000_ICH8_LAN_INIT_TIMEOUT;

    /* Wait for basic configuration completes before proceeding */
    loop {
        data = adapter.read_register(E1000_STATUS);
        data &= E1000_STATUS_LAN_INIT_DONE;
        do_usec_delay(100);
        if data != 0 || count == 0 {
            break;
        }
        count -= 1;
    }
    /* If basic configuration is incomplete before the above loop
     * count reaches 0, loading the configuration from NVM will
     * leave the PHY in a bad state possibly resulting in no link.
     */
    if count == 0 {
        e1000_println!("LAN_INIT_DONE not set, increase timeout");
    }

    /* Clear the Init Done bit for the next init event */
    adapter.clear_register_bit(E1000_STATUS, E1000_STATUS_LAN_INIT_DONE);
}

/// e1000_post_phy_reset_ich8lan - Perform steps required after a PHY reset
/// @hw: pointer to the HW structure
pub fn post_phy_reset_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut reg: u16 = 0;

    match adapter.check_reset_block() {
        Ok(true) => return Ok(()),
        Ok(false) => (),
        Err(e) => eprintln!(e),
    }

    /* Allow time for h/w to get to quiescent state after reset */
    do_msec_delay(10);

    /* Perform any necessary post-reset workarounds */
    if adapter.is_mac(MacType::Mac_pchlan) {
        try!(hv_phy_workarounds_ich8lan(adapter));
    }
    if adapter.is_mac(MacType::Mac_pch2lan) {
        try!(lv_phy_workarounds_ich8lan(adapter));
    }

    /* Clear the host wakeup bit after lcd reset */
    if adapter.hw.mac.mac_type >= MacType::Mac_pchlan {
        adapter.phy_read_reg(BM_PORT_GEN_CFG, &mut reg);
        reg &= !BM_WUC_HOST_WU_BIT as u16;
        adapter.phy_write_reg(BM_PORT_GEN_CFG, reg);
    }

    /* Configure the LCD with the extended configuration region in NVM */
    try!(sw_lcd_config_ich8lan(adapter));

    /* Configure the LCD with the OEM bits in NVM */
    if let Err(e) = oem_bits_config_ich8lan(adapter, true) {
        eprintln!("(IGNORE) {:?}", e);
    }

    if adapter.is_mac(MacType::Mac_pch2lan) {
        /* Ungate automatic PHY configuration on non-managed 82579 */
        if !btst!(adapter.read_register(E1000_FWSM), E1000_ICH_FWSM_FW_VALID) {
            do_msec_delay(10);
            gate_hw_phy_config_ich8lan(adapter, false);
        }
    }

    /* Set EEE LPI Update Timer to 200usec */
    try!(adapter.phy_acquire());
    let res = write_emi_reg_locked(adapter, I82579_LPI_UPDATE_TIMER as u16, 0x1387);
    try!(adapter.phy_release());

    res
}

/// e1000_phy_hw_reset_ich8lan - Performs a PHY reset
/// @hw: pointer to the HW structure
///
/// Resets the PHY
/// This is a function pointer entry point called by drivers
/// or other shared routines.
pub fn phy_hw_reset_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    /* Gate automatic PHY configuration by hardware on non-managed 82579 */
    if adapter.hw.mac.mac_type == MacType::Mac_pch2lan
        && !btst!(adapter.read_register(E1000_FWSM), E1000_ICH_FWSM_FW_VALID)
    {
        gate_hw_phy_config_ich8lan(adapter, true);
    }
    try!(e1000_phy::phy_hw_reset_generic(adapter));

    post_phy_reset_ich8lan(adapter)
}

/// e1000_set_lplu_state_pchlan - Set Low Power Link Up state
/// @hw: pointer to the HW structure
/// @active: TRUE to enable LPLU, FALSE to disable
///
/// Sets the LPLU state according to the active flag.  For PCH, if OEM write
/// bit are disabled in the NVM, writing the LPLU bits in the MAC will not set
/// the phy speed. This function will manually set the LPLU bit and restart
/// auto-neg as hw would do. D3 and D0 LPLU will call the same function
/// since it configures the same bit.
pub fn set_lplu_state_pchlan(adapter: &mut Adapter, active: bool) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_set_d0_lplu_state_ich8lan - Set Low Power Linkup D0 state
/// @hw: pointer to the HW structure
/// @active: TRUE to enable LPLU, FALSE to disable
///
/// Sets the LPLU D0 state according to the active flag.  When
/// activating LPLU this function also disables smart speed
/// and vice versa.  LPLU will not be activated unless the
/// device autonegotiation advertisement meets standards of
/// either 10 or 10/100 or 10/100/1000 at all duplexes.
/// This is a function pointer entry point only called by
/// PHY setup routines.
pub fn set_d0_lplu_state_ich8lan(adapter: &mut Adapter, active: bool) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_set_d3_lplu_state_ich8lan - Set Low Power Linkup D3 state
/// @hw: pointer to the HW structure
/// @active: TRUE to enable LPLU, FALSE to disable
///
/// Sets the LPLU D3 state according to the active flag.  When
/// activating LPLU this function also disables smart speed
/// and vice versa.  LPLU will not be activated unless the
/// device autonegotiation advertisement meets standards of
/// either 10 or 10/100 or 10/100/1000 at all duplexes.
/// This is a function pointer entry point only called by
/// PHY setup routines.
pub fn set_d3_lplu_state_ich8lan(adapter: &mut Adapter, active: bool) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_valid_nvm_bank_detect_ich8lan - finds out the valid bank 0 or 1
/// @hw: pointer to the HW structure
/// @bank:  pointer to the variable that returns the active bank
///
/// Reads signature byte from the NVM using the flash access registers.
/// Word 0x13 bits 15:14 = 10b indicate a valid signature for that bank.
pub fn valid_nvm_bank_detect_ich8lan(adapter: &mut Adapter, bank: &mut u32) -> AdResult {
    e1000_verbose_println!();

    let mut bank1_offset: u32 =
        adapter.hw.nvm.flash_bank_size * kernel::mem::size_of::<u16>() as u32;
    let mut act_offset: u32 = E1000_ICH_NVM_SIG_WORD * 2 + 1;
    let mut nvm_dword: u32 = 0;
    let mut sig_byte: u8 = 0;
    let mut eecd: u32;

    let macs1 = [MacType::Mac_pch_spt, MacType::Mac_pch_cnp];
    let macs2 = [MacType::Mac_ich8lan, MacType::Mac_ich9lan];

    if adapter.is_macs(&macs1) {
        /* set bank to 0 in case flash read fails */
        bank1_offset = adapter.hw.nvm.flash_bank_size;
        act_offset = E1000_ICH_NVM_SIG_WORD;
        *bank = 0;

        /* Check bank 0 */
        try!(read_flash_dword_ich8lan(
            adapter,
            act_offset,
            &mut nvm_dword,
        ));
        sig_byte = ((nvm_dword & 0xFF00) >> 8) as u8;
        if sig_byte & E1000_ICH_NVM_VALID_SIG_MASK as u8 == E1000_ICH_NVM_SIG_VALUE as u8 {
            *bank = 0;
            return Ok(());
        }

        /* Check bank 1 */
        try!(read_flash_dword_ich8lan(
            adapter,
            act_offset + bank1_offset,
            &mut nvm_dword,
        ));
        sig_byte = ((nvm_dword & 0xFF00) >> 8) as u8;
        if sig_byte & E1000_ICH_NVM_VALID_SIG_MASK as u8 == E1000_ICH_NVM_SIG_VALUE as u8 {
            *bank = 1;
            return Ok(());
        }
        return Err("No valid NVM bank present".to_string());
    }
    else if adapter.is_macs(&macs2) {
        eecd = adapter.read_register(E1000_EECD);
        if eecd & E1000_EECD_SEC1VAL_VALID_MASK == E1000_EECD_SEC1VAL_VALID_MASK {
            if btst!(eecd, E1000_EECD_SEC1VAL) {
                *bank = 1;
            } else {
                *bank = 0;
            }
            return Ok(());
        }
        e1000_println!("Unable to determine valid NVM bank via EEC - reading flash signature");
    }
    /* set bank to 0 in case flash read fails */
    *bank = 0;

    /* Check bank 0 */
    try!(read_flash_byte_ich8lan(adapter, act_offset, &mut sig_byte));
    if sig_byte & E1000_ICH_NVM_VALID_SIG_MASK as u8 == E1000_ICH_NVM_SIG_VALUE as u8 {
        *bank = 0;
        return Ok(());
    }

    /* Check bank 1 */
    try!(read_flash_byte_ich8lan(
        adapter,
        act_offset + bank1_offset,
        &mut sig_byte,
    ));
    if sig_byte & E1000_ICH_NVM_VALID_SIG_MASK as u8 == E1000_ICH_NVM_SIG_VALUE as u8 {
        *bank = 1;
        return Ok(());
    }

    Err("No valid NVM bank present".to_string())
}

/// e1000_read_nvm_spt - NVM access for SPT
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the word(s) to read.
/// @words: Size of data to read in words.
/// @data: pointer to the word(s) to read at offset.
///
/// Reads a word(s) from the NVM
pub fn read_nvm_spt(adapter: &mut Adapter, offset: u16, words: u16, data: &mut [u16]) -> AdResult {
    e1000_println!();

    let mut act_offset: u32 = 0;
    let mut bank: u32 = 0;
    let mut dword: u32 = 0;
    let mut offset_to_read: u32 = 0;

    if offset >= adapter.hw.nvm.word_size || words > adapter.hw.nvm.word_size - offset || words == 0
    {
        return Err("nvm parameter(s) out of bounds".to_string());
    }

    try!(
        adapter
            .hw
            .nvm
            .ops
            .acquire
            .ok_or("No function".to_string())
            .and_then(|f| f(adapter))
    );

    try!(valid_nvm_bank_detect_ich8lan(adapter, &mut bank));

    act_offset = match bank > 0 {
        true => adapter.hw.nvm.flash_bank_size,
        false => 0,
    };
    act_offset += offset as u32;

    let mut res = Ok(());
    for i in (0..words as usize).step_by(2) {
        if words - i as u16 == 1 {
            if unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].modified } {
                data[i] =
                    unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].value };
            } else {
                offset_to_read = act_offset + i as u32 - ((act_offset + i as u32) % 2);
                res = read_flash_dword_ich8lan(adapter, offset_to_read, &mut dword);
                if res.is_err() {
                    break;
                }
                if (act_offset + i as u32) % 2 == 0 {
                    data[i] = (dword & 0xFFFF) as u16;
                } else {
                    data[i] = ((dword >> 16) & 0xFFFF) as u16;
                }
            }
        } else {
            offset_to_read = act_offset + i as u32;
            if !unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].modified }
                || !unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].modified }
            {
                res = read_flash_dword_ich8lan(adapter, offset_to_read, &mut dword);
                if res.is_err() {
                    break;
                }
            }
            if unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].modified } {
                data[i] =
                    unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].value };
            } else {
                data[i] = (dword & 0xFFFF) as u16;
            }
            if unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i + 1].modified } {
                data[i + 1] = unsafe {
                    adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i + 1].value
                };
            } else {
                data[i + 1] = (dword & 0xFFFF) as u16;
            }
        }
    }
    try!(
        adapter
            .hw
            .nvm
            .ops
            .release
            .ok_or("No function".to_string())
            .and_then(|f| {
                f(adapter);
                Ok(())
            })
    );

    res
}

/// e1000_read_nvm_ich8lan - Read word(s) from the NVM
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the word(s) to read.
/// @words: Size of data to read in words
/// @data: Pointer to the word(s) to read at offset.
///
/// Reads a word(s) from the NVM using the flash access registers.
pub fn read_nvm_ich8lan(
    adapter: &mut Adapter,
    offset: u16,
    words: u16,
    data: &mut [u16],
) -> AdResult {
    e1000_verbose_println!();

    let mut act_offset: u32;
    let mut bank: u32 = 0;
    let mut word: u16 = 0;

    if offset >= adapter.hw.nvm.word_size || words > (adapter.hw.nvm.word_size - offset)
        || words == 0
    {
        return Err("nvm parameter out of bounds".to_string());
    }

    try!(
        adapter
            .hw
            .nvm
            .ops
            .acquire
            .ok_or("No function".to_string())
            .and_then(|f| f(adapter))
    );

    if let Err(e) = valid_nvm_bank_detect_ich8lan(adapter, &mut bank) {
        e1000_println!("Could not detect valid bank, assuming bank 0");
        bank = 0;
    }

    act_offset = if bank != 0 {
        adapter.hw.nvm.flash_bank_size
    } else {
        0
    };
    act_offset += offset as u32;

    let mut res = Ok(());
    for i in 0..words as usize {
        if unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].modified } {
            data[i] = unsafe { adapter.hw.dev_spec.ich8lan.shadow_ram[offset as usize + i].value };
        } else {
            res = read_flash_word_ich8lan(adapter, act_offset + i as u32, &mut word);
            if res.is_err() {
                break;
            }
            data[i] = word;
        }
    }

    try!(
        adapter
            .hw
            .nvm
            .ops
            .release
            .ok_or("No function".to_string())
            .and_then(|f| {
                f(adapter);
                Ok(())
            })
    );

    res
}

/// e1000_flash_cycle_init_ich8lan - Initialize flash
/// @hw: pointer to the HW structure
///
/// This function does initial flash setup so that a new read/write/erase cycle
/// can be started.
pub fn flash_cycle_init_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_verbose_println!();

    let mut hsfsts: Ich8HwsFlashStatus = Default::default(); // Need to initialize!

    hsfsts.regval = adapter.read_flash_register16(ICH_FLASH_HSFSTS);

    /* Check if the flash descriptor is valid */
    if unsafe { hsfsts.hsf_status.fldesvalid() } == 0 {
        e1000_println!("Flash descriptor invalid. SW Sequencing must be used.");
        return Err("Flash descriptor invalid. SW Sequencing must be used.".to_string());
    }
    /* Clear FCERR and DAEL in hw status by writing 1 */
    unsafe {
        hsfsts.hsf_status.set_flcerr(1);
        hsfsts.hsf_status.set_dael(1);
    }

    if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
        adapter.write_flash_register(ICH_FLASH_HSFSTS, (unsafe { hsfsts.regval } as u32) & 0xFFFF);
    } else {
        adapter.write_flash_register16(ICH_FLASH_HSFSTS, unsafe { hsfsts.regval });
    }

    /* Either we should have a hardware SPI cycle in progress
     *  bit to check against, in order to start a new cycle or
     *  FDONE bit should be changed in the hardware so that it
     *  is 1 after hardware reset, which can then be used as an
     *  indication whether a cycle is in progress or has been
     *  completed.
     */

    if unsafe { hsfsts.hsf_status.flcinprog() } == 0 {
        /* There is no cycle running at present,
         *  so we can start a cycle.
         *  Begin by setting Flash Cycle Done.
         */
        unsafe { hsfsts.hsf_status.set_flcdone(1) };
        if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
            adapter
                .write_flash_register(ICH_FLASH_HSFSTS, (unsafe { hsfsts.regval } as u32) & 0xFFFF);
        } else {
            adapter.write_flash_register16(ICH_FLASH_HSFSTS, unsafe { hsfsts.regval });
        }
    } else {
        /* Otherwise poll for sometime so the current
         *  cycle has a chance to end before giving up.
         */
        let mut status = E1000_ERR_NVM;
        for i in 0..ICH_FLASH_READ_COMMAND_TIMEOUT {
            hsfsts.regval = adapter.read_flash_register16(ICH_FLASH_HSFSTS);
            if unsafe { hsfsts.hsf_status.flcinprog() } == 0 {
                status = E1000_SUCCESS;
                break;
            }
            do_usec_delay(1);
        }
        if status == E1000_SUCCESS {
            /* Successful in waiting for previous cycle to timeout,
             *  now set the Flash Cycle Done.
             */
            unsafe { hsfsts.hsf_status.set_flcdone(1) };
            if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
                adapter.write_flash_register(
                    ICH_FLASH_HSFSTS,
                    (unsafe { hsfsts.regval } as u32) & 0xFFFF,
                );
            } else {
                adapter.write_flash_register16(ICH_FLASH_HSFSTS, unsafe { hsfsts.regval });
            }
        } else {
            return Err("Flash controller busy, cannot get access".to_string());
        }
    }
    Ok(())
}

/// e1000_flash_cycle_ich8lan - Starts flash cycle (read/write/erase)
/// @hw: pointer to the HW structure
/// @timeout: maximum time to wait for completion
///
/// This function starts a flash cycle and waits for its completion.
pub fn flash_cycle_ich8lan(adapter: &mut Adapter, timeout: u32) -> AdResult {
    e1000_verbose_println!();

    let mut hsfsts: Ich8HwsFlashStatus = Default::default(); // Need to initialize!
    let mut hsflctl: Ich8HwsFlashCtrl = Default::default();
    let mut i: u32 = 0;

    /* Start a cycle by writing 1 in Flash Cycle Go in Hw Flash Control */
    if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
        hsflctl.regval = (adapter.read_flash_register(ICH_FLASH_HSFSTS) >> 16) as u16;
    } else {
        hsflctl.regval = adapter.read_flash_register16(ICH_FLASH_HSFCTL);
    }
    unsafe {
        hsflctl.hsf_ctrl.set_flcgo(1);
    }

    if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
        adapter.write_flash_register(ICH_FLASH_HSFSTS, (unsafe { hsflctl.regval } as u32) << 16);
    } else {
        adapter.write_flash_register16(ICH_FLASH_HSFCTL, unsafe { hsflctl.regval });
    }

    /* wait till FDONE bit is set to 1 */
    loop {
        hsfsts.regval = adapter.read_flash_register16(ICH_FLASH_HSFSTS);
        if unsafe { hsfsts.hsf_status.flcdone() } != 0 {
            break;
        }
        do_usec_delay(1);
        if i == timeout {
            break;
        }
        i += 1;
    }
    unsafe {
        if hsfsts.hsf_status.flcdone() != 0 && hsfsts.hsf_status.flcerr() == 0 {
            Ok(())
        } else {
            Err("Flash cycle timeout".to_string())
        }
    }
}

/// e1000_read_flash_dword_ich8lan - Read dword from flash
/// @hw: pointer to the HW structure
/// @offset: offset to data location
/// @data: pointer to the location for storing the data
///
/// Reads the flash dword at offset into data.  Offset is converted
/// to bytes before read.
pub fn read_flash_dword_ich8lan(adapter: &mut Adapter, offset: u32, data: &mut u32) -> AdResult {
    e1000_verbose_println!();

    /* Must convert word offset into bytes. */
    let shifted_offset = offset << 1;

    read_flash_data32_ich8lan(adapter, shifted_offset, data)
}

/// e1000_read_flash_word_ich8lan - Read word from flash
/// @hw: pointer to the HW structure
/// @offset: offset to data location
/// @data: pointer to the location for storing the data
///
/// Reads the flash word at offset into data.  Offset is converted
/// to bytes before read.
pub fn read_flash_word_ich8lan(adapter: &mut Adapter, offset: u32, data: &mut u16) -> AdResult {
    e1000_verbose_println!();

    /* Must convert offset into bytes. */
    let shifted_offset = offset << 1;

    read_flash_data_ich8lan(adapter, shifted_offset, 2, data)
}

/// e1000_read_flash_byte_ich8lan - Read byte from flash
/// @hw: pointer to the HW structure
/// @offset: The offset of the byte to read.
/// @data: Pointer to a byte to store the value read.
///
/// Reads a single byte from the NVM using the flash access registers.
pub fn read_flash_byte_ich8lan(adapter: &mut Adapter, offset: u32, data: &mut u8) -> AdResult {
    e1000_verbose_println!();

    let mut word: u16 = 0;

    /* In SPT, only 32 bits access is supported,
     * so this function should not be called.
     */
    if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
        return Err("Only 32 bit access supported in SPT".to_string());
    }

    try!(read_flash_data_ich8lan(adapter, offset, 1, &mut word));

    *data = word as u8;

    Ok(())
}

/// e1000_read_flash_data_ich8lan - Read byte or word from NVM
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the byte or word to read.
/// @size: Size of data to read, 1=byte 2=word
/// @data: Pointer to the word to store the value read.
///
/// Reads a byte or word from the NVM using the flash access registers.
pub fn read_flash_data_ich8lan(
    adapter: &mut Adapter,
    offset: u32,
    size: u32,
    data: &mut u16,
) -> AdResult {
    e1000_verbose_println!();

    let mut hsfsts: Ich8HwsFlashStatus = Default::default(); // Need to initialize!
    let mut hsflctl: Ich8HwsFlashCtrl = Default::default();
    let mut flash_linear_addr: u32;
    let mut flash_data: u32 = 0;
    let mut count: u8 = 0;

    if size < 1 || size > 2 || offset > ICH_FLASH_LINEAR_ADDR_MASK {
        return Err("Size or offset out of bounds".to_string());
    }

    flash_linear_addr = (ICH_FLASH_LINEAR_ADDR_MASK & offset) + adapter.hw.nvm.flash_base_addr;

    loop {
        do_usec_delay(1);

        /* Steps */
        if let Err(e) = flash_cycle_init_ich8lan(adapter) {
            eprintln!("{:?}", e);
            break;
        }

        hsflctl.regval = adapter.read_flash_register16(ICH_FLASH_HSFCTL);

        /* 0b/1b corresponds to 1 or 2 byte size, respectively. */
        unsafe {
            hsflctl.hsf_ctrl.set_fldbcount(size as u16 - 1);
            hsflctl.hsf_ctrl.set_flcycle(ICH_CYCLE_READ as u16);
        }

        adapter.write_flash_register16(ICH_FLASH_HSFCTL, unsafe { hsflctl.regval });
        adapter.write_flash_register(ICH_FLASH_FADDR, flash_linear_addr);

        if flash_cycle_ich8lan(adapter, ICH_FLASH_READ_COMMAND_TIMEOUT).is_ok() {
            /* Check if FCERR is set to 1, if set to 1, clear it
             *  and try the whole sequence a few more times, else
             *  read in (shift in) the Flash Data0, the order is
             *  least significant byte first msb to lsb
             */
            flash_data = adapter.read_flash_register(ICH_FLASH_FDATA0);
            if size == 1 {
                *data = (flash_data & 0x000000FF) as u16;
            } else if size == 2 {
                *data = (flash_data & 0x0000FFFF) as u16;
            }
            break;
        } else {
            /* If we've gotten here, then things are probably
             *  completely hosed, but if the error condition is
             *  detected, it won't hurt to give it another try...
             *  ICH_FLASH_CYCLE_REPEAT_COUNT times.
             */
            hsfsts.regval = adapter.read_flash_register16(ICH_FLASH_HSFSTS);
            if unsafe { hsfsts.hsf_status.flcerr() } != 0 {
                continue;
            } else if unsafe { hsfsts.hsf_status.flcdone() } == 0 {
                e1000_println!("Timeout error - flash cycle did not complete");
                break;
            }
        }
        if count == ICH_FLASH_CYCLE_REPEAT_COUNT as u8 {
            break;
        }
        count += 1;
    }
    Ok(())
}

/// e1000_read_flash_data32_ich8lan - Read dword from NVM
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the dword to read.
/// @data: Pointer to the dword to store the value read.
///
/// Reads a byte or word from the NVM using the flash access registers.
pub fn read_flash_data32_ich8lan(adapter: &mut Adapter, offset: u32, data: &mut u32) -> AdResult {
    e1000_verbose_println!();

    let mut hsfsts: Ich8HwsFlashStatus = Default::default(); // Need to initialize!
    let mut hsflctl: Ich8HwsFlashCtrl = Default::default();
    let mut flash_linear_addr: u32;
    let mut count: u8 = 0;

    if offset > ICH_FLASH_LINEAR_ADDR_MASK || adapter.hw.mac.mac_type < MacType::Mac_pch_spt {
        return Err("Offset too large or unsupported hardware".to_string());
    }

    flash_linear_addr = (ICH_FLASH_LINEAR_ADDR_MASK & offset) + adapter.hw.nvm.flash_base_addr;

    loop {
        do_usec_delay(1);
        /* Steps */
        if let Err(e) = flash_cycle_init_ich8lan(adapter) {
            eprintln!("{:?}", e);
            break;
        }

        /* In SPT, This register is in Lan memory space, not flash.
         *  Therefore, only 32 bit access is supported
         */
        hsflctl.regval = (adapter.read_flash_register(ICH_FLASH_HSFSTS) >> 16) as u16;

        /* 0b/1b corresponds to 1 or 2 byte size, respectively. */
        unsafe {
            hsflctl
                .hsf_ctrl
                .set_fldbcount(kernel::mem::size_of::<u32>() as u16 - 1);
            hsflctl.hsf_ctrl.set_flcycle(ICH_CYCLE_READ as u16);
        }

        /* In SPT, This register is in Lan memory space, not flash.
         *  Therefore, only 32 bit access is supported
         */
        adapter.write_flash_register(ICH_FLASH_HSFSTS, (unsafe { hsflctl.regval } as u32) << 16);
        adapter.write_flash_register(ICH_FLASH_FADDR, flash_linear_addr);

        /* Check if FCERR is set to 1, if set to 1, clear it
         *  and try the whole sequence a few more times, else
         *  read in (shift in) the Flash Data0, the order is
         *  least significant byte first msb to lsb
         */
        if flash_cycle_ich8lan(adapter, ICH_FLASH_READ_COMMAND_TIMEOUT).is_ok() {
            *data = adapter.read_flash_register(ICH_FLASH_FDATA0);
            break;
        } else {
            /* If we've gotten here, then things are probably
             *  completely hosed, but if the error condition is
             *  detected, it won't hurt to give it another try...
             *  ICH_FLASH_CYCLE_REPEAT_COUNT times.
             */
            hsfsts.regval = adapter.read_flash_register16(ICH_FLASH_HSFSTS);

            if unsafe { hsfsts.hsf_status.flcerr() } != 0 {
                continue;
            } else if unsafe { hsfsts.hsf_status.flcdone() } == 0 {
                e1000_println!("Timeout error - flash cycle did not complete");
                break;
            }
        }
        if count == ICH_FLASH_CYCLE_REPEAT_COUNT as u8 {
            break;
        }
        count += 1;
    }
    Ok(())
}

/// e1000_write_nvm_ich8lan - Write word(s) to the NVM
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the word(s) to write.
/// @words: Size of data to write in words
/// @data: Pointer to the word(s) to write at offset.
///
/// Writes a byte or word to the NVM using the flash access registers.
pub fn write_nvm_ich8lan(adapter: &mut Adapter, offset: u16, words: u16, data: &[u16]) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_update_nvm_checksum_spt - Update the checksum for NVM
/// @hw: pointer to the HW structure
///
/// The NVM checksum is updated by calling the generic update_nvm_checksum,
/// which writes the checksum to the shadow ram.  The changes in the shadow
/// ram are then committed to the EEPROM by processing each bank at a time
/// checking for the modified bit and writing only the pending changes.
/// After a successful commit, the shadow ram is cleared and is ready for
/// future writes.
pub fn update_nvm_checksum_spt(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_update_nvm_checksum_ich8lan - Update the checksum for NVM
/// @hw: pointer to the HW structure
///
/// The NVM checksum is updated by calling the generic update_nvm_checksum,
/// which writes the checksum to the shadow ram.  The changes in the shadow
/// ram are then committed to the EEPROM by processing each bank at a time
/// checking for the modified bit and writing only the pending changes.
/// After a successful commit, the shadow ram is cleared and is ready for
/// future writes.
pub fn update_nvm_checksum_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_validate_nvm_checksum_ich8lan - Validate EEPROM checksum
/// @hw: pointer to the HW structure
///
/// Check to see if checksum needs to be fixed by reading bit 6 in word 0x19.
/// If the bit is 0, that the EEPROM had been modified, but the checksum was not
/// calculated, in which case we need to calculate the checksum and set bit 6.
pub fn validate_nvm_checksum_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut data: [u16; 1] = [0];
    let mut word: u16;
    let mut valid_csum_mask: u16;

    /* Read NVM and check Invalid Image CSUM bit.  If this bit is 0,
     * the checksum needs to be fixed.  This bit is an indication that
     * the NVM was prepared by OEM software and did not calculate
     * the checksum...a likely scenario.
     */
    if adapter.is_macs(&[
        MacType::Mac_pch_lpt,
        MacType::Mac_pch_spt,
        MacType::Mac_pch_cnp,
    ]) {
        word = NVM_COMPAT as u16;
        valid_csum_mask = NVM_COMPAT_VALID_CSUM as u16;
    } else {
        word = NVM_FUTURE_INIT_WORD1 as u16;
        valid_csum_mask = NVM_FUTURE_INIT_WORD1_VALID_CSUM as u16;
    }

    try!(adapter.nvm_read(word, 1, &mut data));

    if !btst!(data[0], valid_csum_mask) {
        data[0] |= valid_csum_mask;
        try!(adapter.nvm_write(word, 1, &mut data));
        try!(
            adapter
                .hw
                .nvm
                .ops
                .update
                .ok_or("No function".to_string())
                .and_then(|f| f(adapter))
        );
    }
    e1000_nvm::validate_nvm_checksum_generic(adapter)
}

/// e1000_write_flash_data_ich8lan - Writes bytes to the NVM
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the byte/word to read.
/// @size: Size of data to read, 1=byte 2=word
/// @data: The byte(s) to write to the NVM.
///
/// Writes one/two bytes to the NVM using the flash access registers.
pub fn write_flash_data_ich8lan(
    adapter: &mut Adapter,
    offset: u32,
    size: u32,
    data: u32,
) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_write_flash_data32_ich8lan - Writes 4 bytes to the NVM
/// @hw: pointer to the HW structure
/// @offset: The offset (in bytes) of the dwords to read.
/// @data: The 4 bytes to write to the NVM.
///
/// Writes one/two/four bytes to the NVM using the flash access registers.
pub fn write_flash_data32_ich8lan(adapter: &mut Adapter, offset: u32, data: u32) -> AdResult {
    e1000_println!();

    let mut hsfsts: Ich8HwsFlashStatus = Default::default(); // Need to initialize!
    let mut hsflctl: Ich8HwsFlashCtrl = Default::default();
    let mut flash_linear_addr: u32;
    let mut count: u8 = 0;

    if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
        if offset > ICH_FLASH_LINEAR_ADDR_MASK {
            return Err("offset > ICH_FLASH_LINEAR_ADDR_MASK".to_string());
        }
    }

    flash_linear_addr = (ICH_FLASH_LINEAR_ADDR_MASK & offset) + adapter.hw.nvm.flash_base_addr;

    loop {
        do_usec_delay(1);

        /* Steps */
        if flash_cycle_init_ich8lan(adapter).is_err() {
            break;
        }

        /* In SPT, This register is in Lan memory space, not
         *  flash.  Therefore, only 32 bit access is supported
         */
        if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
            hsflctl.regval = (adapter.read_flash_register(ICH_FLASH_HSFSTS) >> 16) as u16;
        } else {
            hsflctl.regval = adapter.read_flash_register16(ICH_FLASH_HSFCTL);
        }

        unsafe {
            hsflctl
                .hsf_ctrl
                .set_fldbcount(kernel::mem::size_of::<u32>() as u16 - 1);
            hsflctl.hsf_ctrl.set_flcycle(ICH_CYCLE_WRITE as u16);
        }

        /* In SPT, This register is in Lan memory space,
         *  not flash.  Therefore, only 32 bit access is
         *  supported
         */
        if adapter.hw.mac.mac_type >= MacType::Mac_pch_spt {
            adapter
                .write_flash_register(ICH_FLASH_HSFSTS, (unsafe { hsflctl.regval } as u32) << 16);
        } else {
            adapter.write_flash_register16(ICH_FLASH_HSFCTL, unsafe { hsflctl.regval });
        }

        adapter.write_flash_register(ICH_FLASH_FADDR, flash_linear_addr);
        adapter.write_flash_register(ICH_FLASH_FDATA0, data);

        /* check if FCERR is set to 1 , if set to 1, clear it
         *  and try the whole sequence a few more times else done
         */
        if flash_cycle_ich8lan(adapter, ICH_FLASH_WRITE_COMMAND_TIMEOUT).is_ok() {
            break;
        }

        /* If we're here, then things are most likely
         *  completely hosed, but if the error condition
         *  is detected, it won't hurt to give it another
         *  try...ICH_FLASH_CYCLE_REPEAT_COUNT times.
         */
        hsfsts.regval = adapter.read_flash_register16(ICH_FLASH_HSFSTS);

        if unsafe { hsfsts.hsf_status.flcerr() } != 0 {
            /* Repeat for some time before giving up. */
            continue;
        }
        if unsafe { hsfsts.hsf_status.flcdone() } == 0 {
            e1000_println!("Timeout error - flash cycle did not complete");
            break;
        }
        if count == ICH_FLASH_CYCLE_REPEAT_COUNT as u8 {
            break;
        }
        count += 1;
    }

    Ok(())
}

/// e1000_write_flash_byte_ich8lan - Write a single byte to NVM
/// @hw: pointer to the HW structure
/// @offset: The index of the byte to read.
/// @data: The byte to write to the NVM.
///
/// Writes a single byte to the NVM using the flash access registers.
pub fn write_flash_byte_ich8lan(adapter: &mut Adapter, offset: u32, data: u8) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_retry_write_flash_dword_ich8lan - Writes a dword to NVM
/// @hw: pointer to the HW structure
/// @offset: The offset of the word to write.
/// @dword: The dword to write to the NVM.
///
/// Writes a single dword to the NVM using the flash access registers.
/// Goes through a retry algorithm before giving up.
pub fn retry_write_flash_dword_ich8lan(adapter: &mut Adapter, offset: u32, dword: u32) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_retry_write_flash_byte_ich8lan - Writes a single byte to NVM
/// @hw: pointer to the HW structure
/// @offset: The offset of the byte to write.
/// @byte: The byte to write to the NVM.
///
/// Writes a single byte to the NVM using the flash access registers.
/// Goes through a retry algorithm before giving up.
pub fn retry_write_flash_byte_ich8lan(adapter: &mut Adapter, offset: u32, byte: u8) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_erase_flash_bank_ich8lan - Erase a bank (4k) from NVM
/// @hw: pointer to the HW structure
/// @bank: 0 for first bank, 1 for second bank, etc.
///
/// Erases the bank specified. Each bank is a 4k block. Banks are 0 based.
/// bank N is 4096/// N + flash_reg_addr.
pub fn erase_flash_bank_ich8lan(adapter: &mut Adapter, bank: u32) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_valid_led_default_ich8lan - Set the default LED settings
/// @hw: pointer to the HW structure
/// @data: Pointer to the LED settings
///
/// Reads the LED default settings from the NVM to data.  If the NVM LED
/// settings is all 0's or F's, set the LED default to a valid LED default
/// setting.
pub fn valid_led_default_ich8lan(adapter: &mut Adapter, data: &mut [u16]) -> AdResult {
    e1000_println!();

    try!(adapter.nvm_read(NVM_ID_LED_SETTINGS, 1, data));

    if data[0] == ID_LED_RESERVED_0000 || data[0] == ID_LED_RESERVED_FFFF {
        data[0] = ID_LED_DEFAULT_ICH8LAN;
    }
    Ok(())
}

/// e1000_id_led_init_pchlan - store LED configurations
/// @hw: pointer to the HW structure
///
/// PCH does not control LEDs via the LEDCTL register, rather it uses
/// the PHY LED configuration register.
///
/// PCH also does not have an "always on" or "always off" mode which
/// complicates the ID feature.  Instead of using the "on" mode to indicate
/// in ledctl_mode2 the LEDs to use for ID (see e1000_id_led_init_generic()),
/// use "link_up" mode.  The LEDs will still ID on request if there is no
/// link based on logic in e1000_led_[on|off]_pchlan().
pub fn id_led_init_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let ledctl_on = E1000_LEDCTL_MODE_LINK_UP;
    let ledctl_off = E1000_LEDCTL_MODE_LINK_UP | E1000_PHY_LED0_IVRT;
    let mut data: [u16; 1] = [0];
    let mut temp: u16;
    let mut shift: u16;

    /* Get default ID LED modes */
    try!(
        adapter
            .hw
            .nvm
            .ops
            .valid_led_default
            .ok_or("No function".to_string())
            .and_then(|f| f(adapter, &mut data))
    );

    adapter.hw.mac.ledctl_default = adapter.read_register(E1000_LEDCTL);
    adapter.hw.mac.ledctl_mode1 = adapter.hw.mac.ledctl_default;
    adapter.hw.mac.ledctl_mode2 = adapter.hw.mac.ledctl_default;

    for i in 0..4 {
        temp = (data[0] >> (i << 2)) & E1000_LEDCTL_LED0_MODE_MASK as u16;
        shift = i * 5;
        if [ID_LED_ON1_DEF2, ID_LED_ON1_OFF2, ID_LED_ON1_ON2].contains(&temp) {
            adapter.hw.mac.ledctl_mode1 &= !(E1000_PHY_LED0_MASK << shift);
            adapter.hw.mac.ledctl_mode1 |= ledctl_on << shift;
        }
        if [ID_LED_OFF1_DEF2, ID_LED_OFF1_OFF2, ID_LED_OFF1_ON2].contains(&temp) {
            adapter.hw.mac.ledctl_mode1 &= !(E1000_PHY_LED0_MASK << shift);
            adapter.hw.mac.ledctl_mode1 |= ledctl_off << shift;
        }
        if [ID_LED_DEF1_ON2, ID_LED_ON1_ON2, ID_LED_OFF1_ON2].contains(&temp) {
            adapter.hw.mac.ledctl_mode2 &= !(E1000_PHY_LED0_MASK << shift);
            adapter.hw.mac.ledctl_mode2 |= ledctl_on << shift;
        }
        if [ID_LED_DEF1_OFF2, ID_LED_ON1_OFF2, ID_LED_OFF1_OFF2].contains(&temp) {
            adapter.hw.mac.ledctl_mode2 &= !(E1000_PHY_LED0_MASK << shift);
            adapter.hw.mac.ledctl_mode2 |= ledctl_off << shift;
        }
    }
    Ok(())
}

/// e1000_get_bus_info_ich8lan - Get/Set the bus type and width
/// @hw: pointer to the HW structure
///
/// ICH8 use the PCI Express bus, but does not contain a PCI Express Capability
/// register, so the bus width is hard coded.
pub fn get_bus_info_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let res = e1000_mac::get_bus_info_pcie_generic(adapter);

    /* ICH devices are "PCI Express"-ish.  They have
     * a configuration space, but do not contain
     * PCI Express Capability registers, so bus width
     * must be hardcoded.
     */
    if adapter.hw.bus.width == BusWidth::Unknown {
        adapter.hw.bus.width = BusWidth::Width_pcie_x1;
    }
    res
}

/// e1000_reset_hw_ich8lan - Reset the hardware
/// @hw: pointer to the HW structure
///
/// Does a full reset of the hardware which includes a reset of the PHY and
/// MAC.
pub fn reset_hw_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut kum_cfg: [u16; 1] = [0];
    let mut ctrl: u32;
    let mut reg: u32;

    /* Prevent the PCI-E bus from sticking if there is no TLP connection
     * on the last TLP read/write transaction when MAC is reset.
     */
    if let Err(e) = e1000_mac::disable_pcie_master_generic(adapter) {
        e1000_println!("{:?}", e);
    }

    adapter.write_register(E1000_IMC, 0xFFFFFFFF);

    /* Disable the Transmit and Receive units.  Then delay to allow
     * any pending transactions to complete before we hit the MAC
     * with the global reset.
     */
    adapter.write_register(E1000_RCTL, 0);
    adapter.write_register(E1000_TCTL, E1000_TCTL_PSP);
    adapter.write_flush();

    do_msec_delay(10);

    /* Workaround for ICH8 bit corruption issue in FIFO memory */
    if adapter.hw.mac.mac_type == MacType::Mac_ich8lan {
    	/* Set Tx and Rx buffer allocation to 8k apiece. */
        adapter.write_register(E1000_PBA, E1000_PBA_8K);
    	/* Set Packet Buffer Size to 16k. */
        adapter.write_register(E1000_PBS, E1000_PBS_16K);
    }

    if adapter.hw.mac.mac_type == MacType::Mac_pchlan {
    	/* Save the NVM K1 bit setting*/
        try!(adapter.nvm_read(E1000_NVM_K1_CONFIG as u16, 1, &mut kum_cfg,));
        if btst!(kum_cfg[0], E1000_NVM_K1_ENABLE as u16) {
            unsafe {
                adapter.hw.dev_spec.ich8lan.nvm_k1_enabled = true;
            }
        } else {
            unsafe {
                adapter.hw.dev_spec.ich8lan.nvm_k1_enabled = false;
            }
        }
    }

    ctrl = adapter.read_register(E1000_CTRL);

    match adapter.check_reset_block() {
        Ok(true) => (),
        Ok(false) => {
    	    /* Full-chip reset requires MAC and PHY reset at the same
             * time to make sure the interface between MAC and the
             * external PHY is reset.
    	     */
            ctrl |= E1000_CTRL_PHY_RST;
    	    /* Gate automatic PHY configuration by hardware on
             * non-managed 82579
    	     */
            if adapter.hw.mac.mac_type == MacType::Mac_pch2lan
                && !btst!(adapter.read_register(E1000_FWSM), E1000_ICH_FWSM_FW_VALID)
            {
                gate_hw_phy_config_ich8lan(adapter, true);
            }
        }
        Err(e) => {
            e1000_println!("{:?}", e);
        }
    }

    let swflag = acquire_swflag_ich8lan(adapter);
    e1000_println!("Issuing a global reset to ich8lan");
    adapter.write_register(E1000_CTRL, ctrl | E1000_CTRL_RST);
    /* cannot issue a flush here because it hangs the hardware */
    do_msec_delay(20);

    /* Set Phy Config Counter to 50msec */
    if adapter.hw.mac.mac_type == MacType::Mac_pch2lan {
        reg = adapter.read_register(E1000_FEXTNVM3);
        reg &= !E1000_FEXTNVM3_PHY_CFG_COUNTER_MASK;
        reg |= E1000_FEXTNVM3_PHY_CFG_COUNTER_50MSEC;
        adapter.write_register(E1000_FEXTNVM3, reg);
    }

    if btst!(ctrl, E1000_CTRL_PHY_RST) {
        try!(
            adapter
                .hw
                .phy
                .ops
                .get_cfg_done
                .ok_or("No function".to_string())
                .and_then(|f| f(adapter))
        );
        try!(post_phy_reset_ich8lan(adapter));
    }

    /* For PCH, this write will make sure that any noise
     * will be detected as a CRC error and be dropped rather than show up
     * as a bad packet to the DMA engine.
     */
    if adapter.hw.mac.mac_type == MacType::Mac_pchlan {
        adapter.write_register(E1000_CRC_OFFSET, 0x65656565);
    }

    adapter.write_register(E1000_IMC, 0xFFFFFFFF);
    adapter.read_register(E1000_ICR);

    adapter.set_register_bit(E1000_KABGTXD, E1000_KABGTXD_BGSQLBIAS);

    Ok(())
}

/// e1000_init_hw_ich8lan - Initialize the hardware
/// @hw: pointer to the HW structure
///
/// Prepares the hardware for transmit and receive by doing the following:
/// - initialize hardware bits
/// - initialize LED identification
/// - setup receive address registers
/// - setup flow control
/// - setup transmit descriptors
/// - clear statistics
pub fn init_hw_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut ctrl_ext: u32;
    let mut txdctl: u32;
    let mut snoop: u32;

    initialize_hw_bits_ich8lan(adapter);

    /* Initialize identification LED */
    if let Err(e) = adapter
        .hw
        .mac
        .ops
        .id_led_init
        .ok_or("No function".to_string())
        .and_then(|f| f(adapter))
    {
        e1000_println!("{:?}", e);
    }

    /* Setup the receive address. */
    if let Err(e) =
        e1000_mac::init_rx_addrs_generic(adapter, adapter.hw.mac.rar_entry_count as usize)
    {
        e1000_println!("{:?}", e);
    }

    /* Zero out the Multicast HASH table */
    for i in 0..adapter.hw.mac.mta_reg_count as u32 {
        do_write_register_array(adapter, E1000_MTA, i, 0);
    }

    /* The 82578 Rx buffer will stall if wakeup is enabled in host and
     * the ME.  Disable wakeup by clearing the host wakeup bit.
     * Reset the phy after disabling host wakeup to reset the Rx buffer.
     */
    if adapter.hw.phy.phy_type == PhyType::Type_82578 {
        unsupported!();
    }

    /* Setup link and flow control */
    let setup_link_result = adapter
        .hw
        .mac
        .ops
        .setup_link
        .ok_or("No function".to_string())
        .and_then(|f| f(adapter));

    /* Set the transmit descriptor write-back policy for both queues */
    txdctl = adapter.read_register(E1000_TXDCTL(0));
    txdctl = (txdctl & !E1000_TXDCTL_WTHRESH) | E1000_TXDCTL_FULL_TX_DESC_WB;
    txdctl = (txdctl & !E1000_TXDCTL_PTHRESH) | E1000_TXDCTL_MAX_TX_DESC_PREFETCH;
    adapter.write_register(E1000_TXDCTL(0), txdctl);

    txdctl = adapter.read_register(E1000_TXDCTL(1));
    txdctl = (txdctl & !E1000_TXDCTL_WTHRESH) | E1000_TXDCTL_FULL_TX_DESC_WB;
    txdctl = (txdctl & !E1000_TXDCTL_PTHRESH) | E1000_TXDCTL_MAX_TX_DESC_PREFETCH;
    adapter.write_register(E1000_TXDCTL(1), txdctl);

    /* ICH8 has opposite polarity of no_snoop bits.
     * By default, we should use snoop behavior.
     */
    if adapter.hw.mac.mac_type == MacType::Mac_ich8lan {
        snoop = PCIE_ICH8_SNOOP_ALL;
    } else {
        snoop = (!PCIE_NO_SNOOP_ALL) as u32;
    }

    e1000_mac::set_pcie_no_snoop_generic(adapter, snoop);

    adapter.set_register_bit(E1000_CTRL_EXT, E1000_CTRL_EXT_RO_DIS);

    /* Clear all of the statistics registers (clear on read).  It is
     * important that we do this after we have tried to establish link
     * because the symbol error count will increment wildly if there
     * is no link.
     */
    try!(clear_hw_cntrs_ich8lan(adapter));

    setup_link_result
}

/// e1000_initialize_hw_bits_ich8lan - Initialize required hardware bits
/// @hw: pointer to the HW structure
///
/// Sets/Clears required hardware bits necessary for correctly setting up the
/// hardware for transmit and receive.
pub fn initialize_hw_bits_ich8lan(adapter: &mut Adapter) {
    e1000_println!();

    let mut reg: u32;

    /* Extended Device Control */
    reg = adapter.read_register(E1000_CTRL_EXT);
    reg |= (1 << 22);
    /* Enable PHY low-power state when MAC is at D3 w/o WoL */
    if adapter.is_mac(MacType::Mac_pchlan) {
        reg |= E1000_CTRL_EXT_PHYPDEN;
    }
    adapter.write_register(E1000_CTRL_EXT, reg);

    /* Transmit Descriptor Control 0 */
    adapter.set_register_bit(E1000_TXDCTL(0), 1 << 22);

    /* Transmit Descriptor Control 1 */
    adapter.set_register_bit(E1000_TXDCTL(1), 1 << 22);

    /* Transmit Arbitration Control 0 */
    reg = adapter.read_register(E1000_TARC(0));
    if adapter.is_mac(MacType::Mac_ich8lan) {
        reg |= (1 << 28) | (1 << 29);
    }
    reg |= (1 << 23) | (1 << 24) | (1 << 26) | (1 << 27);
    adapter.write_register(E1000_TARC(0), reg);

    /* Transmit Arbitration Control 1 */
    reg = adapter.read_register(E1000_TARC(1));
    if btst!(adapter.read_register(E1000_TCTL), E1000_TCTL_MULR) {
        reg &= !(1 << 28);
    } else {
        reg |= 1 << 28;
    }
    reg |= (1 << 24) | (1 << 26) | (1 << 30);
    adapter.write_register(E1000_TARC(1), reg);

    /* Device Status */
    if adapter.is_mac(MacType::Mac_ich8lan) {
        adapter.clear_register_bit(E1000_STATUS, 1u32 << 31);
    }

    /* work-around descriptor data corruption issue during nfs v2 udp
     * traffic, just disable the nfs filtering capability
     */
    reg = adapter.read_register(E1000_RFCTL);
    reg |= E1000_RFCTL_NFSW_DIS | E1000_RFCTL_NFSR_DIS;

    /* Disable IPv6 extension header parsing because some malformed
     * IPv6 headers can hang the Rx.
     */
    if adapter.is_mac(MacType::Mac_ich8lan) {
        reg |= E1000_RFCTL_IPV6_EX_DIS | E1000_RFCTL_NEW_IPV6_EXT_DIS;
    }

    adapter.write_register(E1000_RFCTL, reg);

    /* Enable ECC on Lynxpoint */
    if adapter.hw.mac.mac_type >= MacType::Mac_pch_lpt {
        adapter.set_register_bit(E1000_PBECCSTS, E1000_PBECCSTS_ECC_ENABLE);
        adapter.set_register_bit(E1000_CTRL, E1000_CTRL_MEHE);
    }
}

/// e1000_setup_link_ich8lan - Setup flow control and link settings
/// @hw: pointer to the HW structure
///
/// Determines which flow control settings to use, then configures flow
/// control.  Calls the appropriate media-specific link configuration
/// function.  Assuming the adapter has a valid link partner, a valid link
/// should be established.  Assumes the hardware has previously been reset
/// and the transmitter and receiver are not enabled.
pub fn setup_link_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    match adapter.check_reset_block() {
        Ok(true) => return Ok(()),
        Ok(false) => (),
        Err(e) => eprintln!("{:?}", e),
    }

    /* ICH parts do not have a word in the NVM to determine
     * the default flow control setting, so we explicitly
     * set it to full.
     */
    if adapter.hw.fc.requested_mode == FcMode::Default {
        adapter.hw.fc.requested_mode = FcMode::Full;
    }

    /* Save off the requested flow control mode for use later.  Depending
     * on the link partner's capabilities, we may or may not use this mode.
     */
    adapter.hw.fc.current_mode = adapter.hw.fc.requested_mode;

    e1000_println!(
        "After fix-ups FlowControl is now {:?}",
        adapter.hw.fc.current_mode
    );

    /* Continue to configure the copper link. */
    try!(
        adapter
            .hw
            .mac
            .ops
            .setup_physical_interface
            .ok_or("No function".to_string())
            .and_then(|f| f(adapter))
    );

    adapter.write_register(E1000_FCTTV, adapter.hw.fc.pause_time as u32);

    if [
        PhyType::Type_82578,
        PhyType::Type_82579,
        PhyType::Type_i217,
        PhyType::Type_82577,
    ].contains(&adapter.hw.phy.phy_type)
    {
        adapter.write_register(E1000_FCRTV_PCH, adapter.hw.fc.refresh_time as u32);
        try!(adapter.phy_write_reg(fn_phy_reg(BM_PORT_CTRL_PAGE, 27), adapter.hw.fc.pause_time,));
    }
    e1000_mac::set_fc_watermarks_generic(adapter);
    Ok(())
}

/// e1000_setup_copper_link_ich8lan - Configure MAC/PHY interface
/// @hw: pointer to the HW structure
///
/// Configures the kumeran interface to the PHY to wait the appropriate time
/// when polling the PHY, then call the generic setup_copper_link to finish
/// configuring the copper link.
pub fn setup_copper_link_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_setup_copper_link_pch_lpt - Configure MAC/PHY interface
/// @hw: pointer to the HW structure
///
/// Calls the PHY specific link setup function and then calls the
/// generic setup_copper_link to finish configuring the link for
/// Lynxpoint PCH devices
pub fn setup_copper_link_pch_lpt(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut ctrl: u32;

    ctrl = adapter.read_register(E1000_CTRL);
    ctrl |= E1000_CTRL_SLU;
    ctrl &= !(E1000_CTRL_FRCSPD | E1000_CTRL_FRCDPX);
    adapter.write_register(E1000_CTRL, ctrl);

    try!(e1000_phy::copper_link_setup_82577(adapter));

    e1000_phy::setup_copper_link_generic(adapter)
}

/// e1000_get_link_up_info_ich8lan - Get current link speed and duplex
/// @hw: pointer to the HW structure
/// @speed: pointer to store current link speed
/// @duplex: pointer to store the current link duplex
///
/// Calls the generic get_speed_and_duplex to retrieve the current link
/// information and then calls the Kumeran lock loss workaround for links at
/// gigabit speeds.
pub fn get_link_up_info_ich8lan(
    adapter: &mut Adapter,
    speed: &mut u16,
    duplex: &mut u16,
) -> AdResult {
    e1000_println!();

    try!(e1000_mac::get_speed_and_duplex_copper_generic(
        adapter,
        speed,
        duplex,
    ));

    if adapter.hw.mac.mac_type == MacType::Mac_ich8lan
        && adapter.hw.phy.phy_type == PhyType::Type_igp_3
    {
        unsupported!();
        incomplete_return!();
    }
    Ok(())
}

/// e1000_kmrn_lock_loss_workaround_ich8lan - Kumeran workaround
/// @hw: pointer to the HW structure
///
/// Work-around for 82566 Kumeran PCS lock loss:
/// On link status change (i.e. PCI reset, speed change) and link is up and
/// speed is gigabit-
/// 0) if workaround is optionally disabled do nothing
/// 1) wait 1ms for Kumeran link to come up
/// 2) check Kumeran Diagnostic register PCS lock loss bit
/// 3) if not set the link is locked (all is good), otherwise...
/// 4) reset the PHY
/// 5) repeat up to 10 times
/// Note: this is only called for IGP3 copper when speed is 1gb.
pub fn kmrn_lock_loss_workaround_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();
    incomplete_return!();
}

/// e1000_set_kmrn_lock_loss_workaround_ich8lan - Set Kumeran workaround state
/// @hw: pointer to the HW structure
/// @state: boolean value used to set the current Kumeran workaround state
///
/// If ICH8, set the current Kumeran workaround state (enabled - TRUE
/// /disabled - FALSE).
pub fn set_kmrn_lock_loss_workaround_ich8lan(adapter: &mut Adapter, state: bool) {
    e1000_println!();
    incomplete!();
}

/// e1000_ipg3_phy_powerdown_workaround_ich8lan - Power down workaround on D3
/// @hw: pointer to the HW structure
///
/// Workaround for 82566 power-down on D3 entry:
/// 1) disable gigabit link
/// 2) write VR power-down enable
/// 3) read it back
/// Continue if successful, else issue LCD reset and repeat
pub fn igp3_phy_powerdown_workaround_ich8lan(adapter: &mut Adapter) {
    e1000_println!();
    incomplete!();
}

/// e1000_gig_downshift_workaround_ich8lan - WoL from S5 stops working
/// @hw: pointer to the HW structure
///
/// Steps to take when dropping from 1Gb/s (eg. link cable removal (LSC),
/// LPLU, Gig disable, MDIC PHY reset):
/// 1) Set Kumeran Near-end loopback
/// 2) Clear Kumeran Near-end loopback
/// Should only be called for ICH8[m] devices with any 1G Phy.
pub fn gig_downshift_workaround_ich8lan(adapter: &mut Adapter) {
    e1000_println!();
    incomplete!();
}

/// e1000_suspend_workarounds_ich8lan - workarounds needed during S0->Sx
/// @hw: pointer to the HW structure
///
/// During S0 to Sx transition, it is possible the link remains at gig
/// instead of negotiating to a lower speed.  Before going to Sx, set
/// 'Gig Disable' to force link speed negotiation to a lower speed based on
/// the LPLU setting in the NVM or custom setting.  For PCH and newer parts,
/// the OEM bits PHY register (LED, GbE disable and LPLU configurations) also
/// needs to be written.
/// Parts that support (and are linked to a partner which support) EEE in
/// 100Mbps should disable LPLU since 100Mbps w/ EEE requires less power
/// than 10Mbps w/o EEE.
pub fn suspend_workarounds_ich8lan(adapter: &mut Adapter) {
    e1000_println!();
    incomplete!();
}

/// e1000_resume_workarounds_pchlan - workarounds needed during Sx->S0
/// @hw: pointer to the HW structure
///
/// During Sx to S0 transitions on non-managed devices or managed devices
/// on which PHY resets are not blocked, if the PHY registers cannot be
/// accessed properly by the s/w toggle the LANPHYPC value to power cycle
/// the PHY.
/// On i217, setup Intel Rapid Start Technology.
pub fn resume_workarounds_pchlan(adapter: &mut Adapter) -> u32 {
    e1000_println!();
    incomplete!();
    0
}

/// e1000_cleanup_led_ich8lan - Restore the default LED operation
/// @hw: pointer to the HW structure
///
/// Return the LED back to the default configuration.
pub fn cleanup_led_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    if adapter.hw.phy.phy_type == PhyType::Type_ife {
        return adapter.phy_write_reg(IFE_PHY_SPECIAL_CONTROL_LED, 0);
    }
    adapter.write_register(E1000_LEDCTL, adapter.hw.mac.ledctl_default);
    Ok(())
}

/// e1000_led_on_ich8lan - Turn LEDs on
/// @hw: pointer to the HW structure
///
/// Turn on the LEDs.
pub fn led_on_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    if adapter.hw.phy.phy_type == PhyType::Type_ife {
        return adapter.phy_write_reg(
            IFE_PHY_SPECIAL_CONTROL_LED,
            (IFE_PSCL_PROBE_MODE | IFE_PSCL_PROBE_LEDS_ON) as u16,
        );
    }

    adapter.write_register(E1000_LEDCTL, adapter.hw.mac.ledctl_mode2);
    Ok(())
}

/// e1000_led_off_ich8lan - Turn LEDs off
/// @hw: pointer to the HW structure
///
/// Turn off the LEDs.
pub fn led_off_ich8lan(adapter: &mut Adapter) -> AdResult {

    e1000_println!();

    if adapter.hw.phy.phy_type == PhyType::Type_ife {
        return adapter.phy_write_reg(
            IFE_PHY_SPECIAL_CONTROL_LED,
            (IFE_PSCL_PROBE_MODE | IFE_PSCL_PROBE_LEDS_OFF) as u16,
        );
    }

    adapter.write_register(E1000_LEDCTL, adapter.hw.mac.ledctl_mode1);
    Ok(())
}

/// e1000_setup_led_pchlan - Configures SW controllable LED
/// @hw: pointer to the HW structure
///
/// This prepares the SW controllable LED for use.
pub fn setup_led_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    adapter.phy_write_reg(HV_LED_CONFIG, adapter.hw.mac.ledctl_mode1 as u16)
}

/// e1000_cleanup_led_pchlan - Restore the default LED operation
/// @hw: pointer to the HW structure
///
/// Return the LED back to the default configuration.
pub fn cleanup_led_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    adapter.phy_write_reg(HV_LED_CONFIG, adapter.hw.mac.ledctl_default as u16)
}

/// e1000_led_on_pchlan - Turn LEDs on
/// @hw: pointer to the HW structure
///
/// Turn on the LEDs.
pub fn led_on_pchlan(adapter: &mut Adapter) -> AdResult {

    e1000_println!();

    let mut data: u16 = adapter.hw.mac.ledctl_mode2 as u16;
    let mut led: u32;
    /* If no link, then turn LED on by setting the invert bit
     * for each LED that's mode is "link_up" in ledctl_mode2.
     */
    if !btst!(adapter.read_register(E1000_STATUS), E1000_STATUS_LU) {
        for i in 0..3 {
            led = (data as u32) >> (i * 5);
            if led & E1000_PHY_LED0_MODE_MASK != E1000_LEDCTL_LED0_MODE_MASK {
                continue;
            }
            if btst!(led, E1000_PHY_LED0_IVRT) {
                data &= !((E1000_PHY_LED0_IVRT as u16) << (i * 5));
            } else {
                data |= ((E1000_PHY_LED0_IVRT as u16) << (i * 5));
            }
        }
    }
    adapter.phy_write_reg(HV_LED_CONFIG, data)
}

/// e1000_led_off_pchlan - Turn LEDs off
/// @hw: pointer to the HW structure
///
/// Turn off the LEDs.
pub fn led_off_pchlan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut data: u16 = adapter.hw.mac.ledctl_mode1 as u16;
    let mut led: u32;

    /* If no link, then turn LED off by clearing the invert bit
     * for each LED that's mode is "link_up" in ledctl_mode1.
     */
    if !btst!(adapter.read_register(E1000_STATUS), E1000_STATUS_LU) {
        for i in 0..3 {
            led = (data as u32 >> (i * 5)) & E1000_PHY_LED0_MASK;
            if led & E1000_PHY_LED0_MODE_MASK != E1000_LEDCTL_MODE_LINK_UP {
                continue;
            }
            if btst!(led, E1000_PHY_LED0_IVRT as u32) {
                data &= !((E1000_PHY_LED0_IVRT as u16) << (i * 5));
            } else {
                data |= (E1000_PHY_LED0_IVRT as u16) << (i * 5);
            }
        }
    }
    adapter.phy_write_reg(HV_LED_CONFIG, data)
}

/// e1000_get_cfg_done_ich8lan - Read config done bit after Full or PHY reset
/// @hw: pointer to the HW structure
///
/// Read appropriate register for the config done bit for completion status
/// and configure the PHY through s/w for EEPROM-less parts.
///
/// NOTE: some silicon which is EEPROM-less will fail trying to read the
/// config done bit, so only an error is logged and continues.  If we were
/// to return with error, EEPROM-less silicon would not be able to be reset
/// or change link.
pub fn get_cfg_done_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_verbose_println!();

    if let Err(e) = e1000_phy::get_cfg_done_generic(adapter) {
        eprintln!("(IGNORE) {:?}", e);
    }

    /* Wait for indication from h/w that it has completed basic config */
    if adapter.hw.mac.mac_type >= MacType::Mac_ich10lan {
        lan_init_done_ich8lan(adapter);
    } else {
        match e1000_mac::get_auto_rd_done_generic(adapter) {
    	    /* When auto config read does not complete, do not
    	     * return with an error. This can happen in situations
    	     * where there is no eeprom and prevents getting link.
    	     */
            Ok(_) => (),
            Err(e) => {
                eprintln!("{:?}", e);
                eprintln!("^ Ignore error and return OK");
                return Ok(());
            }
        }
    }

    /* Clear PHY Reset Asserted bit */
    let status = adapter.read_register(E1000_STATUS);
    if btst!(status, E1000_STATUS_PHYRA) {
        adapter.write_register(E1000_STATUS, status & !E1000_STATUS_PHYRA);
    } else {
        e1000_println!("PHY Reset Asserted not set - needs delay");
    }

    /* If EEPROM is not marked present, init the IGP 3 PHY manually */
    let mut bank: u32 = 0;
    if adapter.hw.mac.mac_type <= MacType::Mac_ich9lan {
        if !btst!(adapter.read_register(E1000_EECD), E1000_EECD_PRES)
            && adapter.hw.phy.phy_type == PhyType::Type_igp_3
        {
            eprintln!("Need function e1000_phy_init_script_igp3(hw)");
            incomplete_return!();
        }
    } else {
        try!(valid_nvm_bank_detect_ich8lan(adapter, &mut bank));
    	/* Maybe we should do a basic PHY config */
    }

    Ok(())
}

/// e1000_power_down_phy_copper_ich8lan - Remove link during PHY power down
/// @hw: pointer to the HW structure
///
/// In the case of a PHY power down to save power, or to turn off link during a
/// driver unload, or wake on lan is not enabled, remove the link.
pub fn power_down_phy_copper_ich8lan(adapter: &mut Adapter) {
    e1000_println!();
    incomplete!();
}

/// e1000_clear_hw_cntrs_ich8lan - Clear statistical counters
/// @hw: pointer to the HW structure
///
/// Clears hardware counters specific to the silicon family and calls
/// clear_hw_cntrs_generic to clear all general purpose counters.
pub fn clear_hw_cntrs_ich8lan(adapter: &mut Adapter) -> AdResult {
    e1000_println!();

    let mut phy_data: u16 = 0;

    e1000_mac::clear_hw_cntrs_base_generic(adapter);

    adapter.read_register(E1000_ALGNERRC);
    adapter.read_register(E1000_RXERRC);
    adapter.read_register(E1000_TNCRS);
    adapter.read_register(E1000_CEXTERR);
    adapter.read_register(E1000_TSCTC);
    adapter.read_register(E1000_TSCTFC);
    adapter.read_register(E1000_MGTPRC);
    adapter.read_register(E1000_MGTPDC);
    adapter.read_register(E1000_MGTPTC);
    adapter.read_register(E1000_IAC);
    adapter.read_register(E1000_ICRXOC);

    /* Clear PHY statistics registers */
    if [
        PhyType::Type_82578,
        PhyType::Type_82579,
        PhyType::Type_i217,
        PhyType::Type_82577,
    ].contains(&adapter.hw.phy.phy_type)
    {
        try!(adapter.phy_acquire());
        if let Err(e) = adapter
            .hw
            .phy
            .ops
            .set_page
            .ok_or("No function".to_string())
            .and_then(|f| f(adapter, (HV_STATS_PAGE as u16) << IGP_PAGE_SHIFT))
        {
            eprintln!("{:?}", e);
            return adapter.phy_release();
        }
        if let Some(read_reg_page) = adapter.hw.phy.ops.read_reg_page {
            try!(read_reg_page(adapter, HV_SCC_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_SCC_LOWER, &mut phy_data));
            try!(read_reg_page(adapter, HV_ECOL_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_ECOL_LOWER, &mut phy_data));
            try!(read_reg_page(adapter, HV_MCC_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_MCC_LOWER, &mut phy_data));
            try!(read_reg_page(adapter, HV_LATECOL_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_LATECOL_LOWER, &mut phy_data));
            try!(read_reg_page(adapter, HV_COLC_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_COLC_LOWER, &mut phy_data));
            try!(read_reg_page(adapter, HV_DC_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_DC_LOWER, &mut phy_data));
            try!(read_reg_page(adapter, HV_TNCRS_UPPER, &mut phy_data));
            try!(read_reg_page(adapter, HV_TNCRS_LOWER, &mut phy_data));
        }
        try!(adapter.phy_release());
    }
    Ok(())
}
