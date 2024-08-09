use a2lfile::{
    A2lFile, A2lObject, AddrType, Characteristic, CharacteristicType, EcuAddress, FncValues, Group,
    IndexMode, Instance, Measurement, Module, RecordLayout, RefCharacteristic, RefMeasurement,
    Root, SymbolLink,
};
use std::collections::HashMap;

use crate::datatype::{get_a2l_datatype, get_type_limits};
use crate::dwarf::{DebugData, DwarfDataType, TypeInfo};
use crate::symbol::SymbolInfo;
use crate::update::{
    self, enums, make_symbol_link_string, set_address_type, set_bitmask, set_matrix_dim,
};
use crate::A2lVersion;
use regex::Regex;

#[derive(Clone, Copy)]
enum ItemType {
    Measurement(usize),
    Characteristic(usize),
    Instance(usize),
    Blob,
    AxisPts,
}

struct InsertSupport<'a2l, 'dbg, 'param> {
    module: &'a2l mut Module,
    debug_data: &'dbg DebugData,
    compiled_meas_re: Vec<Regex>,
    compiled_char_re: Vec<Regex>,
    measurement_ranges: &'param [(u64, u64)],
    characteristic_ranges: &'param [(u64, u64)],
    name_map: HashMap<String, ItemType>,
    sym_map: HashMap<String, ItemType>,
    characteristic_list: Vec<String>,
    measurement_list: Vec<String>,
    meas_count: u32,
    chara_count: u32,
    instance_count: u32,
    version: A2lVersion,
    create_typedef: Vec<(&'dbg TypeInfo, usize)>,
}

pub(crate) fn insert_items(
    a2l_file: &mut A2lFile,
    debug_data: &DebugData,
    measurement_symbols: Vec<&str>,
    characteristic_symbols: Vec<&str>,
    target_group: Option<&str>,
    log_msgs: &mut Vec<String>,
    enable_structures: bool,
    lower_value: Option<f64>,
    upper_value: Option<f64>,
) {
    let version = A2lVersion::from(&*a2l_file);
    let module = &mut a2l_file.project.module[0];
    let (mut name_map, mut sym_map) = build_maps(&module);
    let mut characteristic_list = vec![];
    let mut measurement_list = vec![];

    let mut insert_list: Vec<(&str, SymbolInfo, bool)> = Vec::new();

    for measure_sym in measurement_symbols {
        match crate::symbol::find_symbol(measure_sym, debug_data) {
            Ok(sym_info) => insert_list.push((measure_sym, sym_info, false)),
            Err(errmsg) => log_msgs.push(format!(
                "Insert skipped: Symbol {measure_sym} could not be added: {errmsg}"
            )),
        }
    }
    for characteristic_sym in characteristic_symbols {
        match crate::symbol::find_symbol(characteristic_sym, debug_data) {
            Ok(sym_info) => insert_list.push((characteristic_sym, sym_info, true)),
            Err(errmsg) => log_msgs.push(format!(
                "Insert skipped: Symbol {characteristic_sym} could not be added: {errmsg}"
            )),
        }
    }

    let mut create_typedef = Vec::new();
    for (sym_name, sym_info, is_calib) in insert_list {
        if is_simple_type(sym_info.typeinfo)
            || sym_info
                .typeinfo
                .get_arraytype()
                .map(is_simple_type)
                .unwrap_or(false)
        {
            if is_calib {
                match insert_characteristic_sym(
                    module,
                    debug_data,
                    sym_name,
                    &sym_info,
                    &name_map,
                    &sym_map,
                    version,
                    lower_value,
                    upper_value,
                ) {
                    Ok(characteristic_name) => {
                        log_msgs.push(format!("Inserted CHARACTERISTIC {characteristic_name}"));
                        characteristic_list.push(characteristic_name.clone());

                        let it = ItemType::Characteristic(module.characteristic.len() - 1);
                        name_map.insert(characteristic_name, it);
                        sym_map.insert(sym_name.to_string(), it);
                    }
                    Err(errmsg) => {
                        log_msgs.push(format!("Insert skipped: {errmsg}"));
                    }
                }
            } else {
                match insert_measurement_sym(
                    module, debug_data, &sym_info, &name_map, &sym_map, version,
                ) {
                    Ok(measure_name) => {
                        log_msgs.push(format!("Inserted MEASUREMENT {measure_name}"));
                        measurement_list.push(measure_name.clone());

                        let it = ItemType::Measurement(module.measurement.len() - 1);
                        name_map.insert(measure_name, it);
                        sym_map.insert(sym_name.to_string(), it);
                    }
                    Err(errmsg) => {
                        log_msgs.push(format!("Insert skipped: {errmsg}"));
                    }
                }
            }
        } else if enable_structures
            && !matches!(sym_info.typeinfo.datatype, DwarfDataType::FuncPtr(_))
        {
            match insert_instance_sym(
                module, debug_data, sym_name, &sym_info, &name_map, &sym_map, is_calib,
            ) {
                Ok((instance_name, typedef_typeinfo)) => {
                    if is_calib {
                        log_msgs.push(format!("Inserted characteristic INSTANCE {instance_name}"));
                        characteristic_list.push(instance_name.clone());
                    } else {
                        log_msgs.push(format!("Inserted measurement INSTANCE {instance_name}"));
                        measurement_list.push(instance_name.clone());
                    }

                    create_typedef.push((typedef_typeinfo, module.instance.len() - 1));

                    let it = ItemType::Instance(module.instance.len() - 1);
                    name_map.insert(instance_name, it);
                    sym_map.insert(sym_name.to_string(), it);
                }
                Err(errmsg) => {
                    log_msgs.push(format!("Insert skipped: {errmsg}"));
                }
            }
        } else {
            log_msgs.push(format!(
                "Insert skipped: Symbol {sym_name} exists but has the unsuitable data type {}",
                sym_info.typeinfo
            ));
        }
    }

    update::typedef::create_new_typedefs(module, debug_data, log_msgs, &create_typedef);

    if let Some(group_name) = target_group {
        create_or_update_group(module, group_name, characteristic_list, measurement_list);
    }
}

fn insert_measurement_sym(
    module: &mut Module,
    debug_data: &DebugData,
    sym_info: &SymbolInfo,
    name_map: &HashMap<String, ItemType>,
    sym_map: &HashMap<String, ItemType>,
    version: A2lVersion,
) -> Result<String, String> {
    // Abort if a MEASUREMENT for this symbol already exists. Warn if any other reference to the symbol exists
    let symbol_link_text = make_symbol_link_string(sym_info, debug_data);
    let item_name = make_unique_measurement_name(module, sym_map, &sym_info.name, name_map)?;

    let datatype = get_a2l_datatype(sym_info.typeinfo);
    let (lower_limit, upper_limit) = get_type_limits(sym_info.typeinfo, f64::MIN, f64::MAX);
    let mut new_measurement = Measurement::new(
        item_name.clone(),
        format!("measurement for symbol {}", sym_info.name),
        datatype,
        "NO_COMPU_METHOD".to_string(),
        0,
        0f64,
        lower_limit,
        upper_limit,
    );
    // create an ECU_ADDRESS attribute, and set it to hex display mode
    let mut ecu_address = EcuAddress::new(sym_info.address as u32);
    ecu_address.get_layout_mut().item_location.0 .1 = true;
    new_measurement.ecu_address = Some(ecu_address);

    // create a SYMBOL_LINK attribute
    if version >= A2lVersion::V1_6_0 {
        new_measurement.symbol_link = Some(SymbolLink::new(symbol_link_text.clone(), 0));
    }

    // handle pointers - only allowed for version 1.7.0+ (the caller should take care of this precondition)
    update::set_address_type(&mut new_measurement.address_type, sym_info.typeinfo);
    let typeinfo = sym_info
        .typeinfo
        .get_pointer(&debug_data.types)
        .map(|(_, t)| t)
        .unwrap_or(sym_info.typeinfo);

    // handle arrays and unwrap the typeinfo
    update::set_matrix_dim(
        &mut new_measurement.matrix_dim,
        typeinfo,
        version >= A2lVersion::V1_7_0,
    );
    let typeinfo = typeinfo.get_arraytype().unwrap_or(typeinfo);

    if let DwarfDataType::Enum { enumerators, .. } = &typeinfo.datatype {
        // create a conversion table for enums
        let enum_name = typeinfo
            .name
            .clone()
            .unwrap_or_else(|| format!("{}_compu_method", new_measurement.name));
        enums::cond_create_enum_conversion(module, &enum_name, enumerators);
        new_measurement.conversion = enum_name;
    } else {
        update::set_bitmask(&mut new_measurement.bit_mask, typeinfo);
    }
    module.measurement.push(new_measurement);

    Ok(item_name)
}

fn insert_characteristic_sym(
    module: &mut Module,
    debug_data: &DebugData,
    characteristic_sym: &str,
    sym_info: &SymbolInfo,
    name_map: &HashMap<String, ItemType>,
    sym_map: &HashMap<String, ItemType>,
    version: A2lVersion,
    lower_value: Option<f64>,
    upper_value: Option<f64>,
) -> Result<String, String> {
    let symbol_link_text = make_symbol_link_string(sym_info, debug_data);
    let item_name = make_unique_characteristic_name(module, sym_map, characteristic_sym, name_map)?;

    let mut matrix_dim = None;
    set_matrix_dim(
        &mut matrix_dim,
        sym_info.typeinfo,
        version >= A2lVersion::V1_7_0,
    );
    let (typeinfo, ctype) = if let Some(arraytype) = sym_info.typeinfo.get_arraytype() {
        (arraytype, CharacteristicType::ValBlk)
    } else {
        (sym_info.typeinfo, CharacteristicType::Value)
    };

    let datatype = get_a2l_datatype(typeinfo);
    let recordlayout_name = format!("__{datatype}_Z");

    let (default_lower_limit, default_upper_limit) = get_type_limits(typeinfo, f64::MIN, f64::MAX);

    let upper_limit = upper_value.unwrap_or(default_upper_limit);
    let lower_limit = lower_value.unwrap_or(default_lower_limit);

    let mut new_characteristic = Characteristic::new(
        item_name.clone(),
        format!("characteristic for {characteristic_sym}"),
        ctype,
        sym_info.address as u32,
        recordlayout_name.clone(),
        0f64,
        "NO_COMPU_METHOD".to_string(),
        lower_limit,
        upper_limit,
    );
    new_characteristic.matrix_dim = matrix_dim;

    set_bitmask(&mut new_characteristic.bit_mask, typeinfo);

    if let DwarfDataType::Enum { enumerators, .. } = &typeinfo.datatype {
        let enum_name = typeinfo
            .name
            .clone()
            .unwrap_or_else(|| format!("{item_name}_compu_method"));
        enums::cond_create_enum_conversion(module, &enum_name, enumerators);
        new_characteristic.conversion = enum_name;
    }

    // enable hex mode for the address (item 3 in the CHARACTERISTIC)
    new_characteristic.get_layout_mut().item_location.3 .1 = true;

    if version >= A2lVersion::V1_6_0 {
        // create a SYMBOL_LINK
        new_characteristic.symbol_link = Some(SymbolLink::new(symbol_link_text.clone(), 0));
    }

    // insert the CHARACTERISTIC into the module's list
    module.characteristic.push(new_characteristic);

    // create a RECORD_LAYOUT for the CHARACTERISTIC if it doesn't exist yet
    // the used naming convention (__<type>_Z) matches default naming used by Vector tools
    let mut recordlayout = RecordLayout::new(recordlayout_name.clone());
    // set item 0 (name) to use an offset of 0 lines, i.e. no line break after /begin RECORD_LAYOUT
    recordlayout.get_layout_mut().item_location.0 = 0;
    recordlayout.fnc_values = Some(FncValues::new(
        1,
        datatype,
        IndexMode::RowDir,
        AddrType::Direct,
    ));
    // search through all existing record layouts and only add the new one if it doesn't exist yet
    if !module
        .record_layout
        .iter()
        .any(|rl| rl.name == recordlayout_name)
    {
        module.record_layout.push(recordlayout);
    }

    Ok(item_name)
}

fn make_unique_measurement_name(
    module: &Module,
    sym_map: &HashMap<String, ItemType>,
    measure_sym: &str,
    name_map: &HashMap<String, ItemType>,
) -> Result<String, String> {
    // ideally the item name is the symbol name.
    // if the symbol is a demangled c++ symbol, then it might contain a "::", e.g. namespace::variable
    let cleaned_sym = measure_sym.replace("::", "__");

    // If an object of a different type already has this name, add the prefix "CHARACTERISTIC."
    let item_name = match sym_map.get(&cleaned_sym) {
        Some(ItemType::Measurement(idx)) => {
            return Err(format!(
                "MEASUREMENT {} already references symbol {}.",
                module.measurement[*idx].name, measure_sym
            ))
        }
        Some(
            ItemType::Characteristic(_)
            | ItemType::Instance(_)
            | ItemType::Blob
            | ItemType::AxisPts,
        ) => {
            format!("MEASUREMENT.{cleaned_sym}")
        }
        None => cleaned_sym,
    };
    // fail if the name still isn't unique
    if name_map.get(&item_name).is_some() {
        return Err(format!("MEASUREMENT {item_name} already exists."));
    }
    Ok(item_name)
}

fn make_unique_characteristic_name(
    module: &Module,
    sym_map: &HashMap<String, ItemType>,
    characteristic_sym: &str,
    name_map: &HashMap<String, ItemType>,
) -> Result<String, String> {
    // ideally the item name is the symbol name.
    // if the symbol is a demangled c++ symbol, then it might contain a "::", e.g. namespace::variable
    let cleaned_sym = characteristic_sym.replace("::", "__");

    // If an object of a different type already has this name, add the prefix "CHARACTERISTIC."
    let item_name = match sym_map.get(&cleaned_sym) {
        Some(ItemType::Characteristic(idx)) => {
            return Err(format!(
                "CHARACTERISTIC {} already references symbol {}.",
                module.characteristic[*idx].name, characteristic_sym
            ))
        }
        Some(
            ItemType::Measurement(_) | ItemType::Instance(_) | ItemType::Blob | ItemType::AxisPts,
        ) => {
            format!("CHARACTERISTIC.{cleaned_sym}")
        }
        None => cleaned_sym,
    };
    // fail if the name still isn't unique
    if name_map.get(&item_name).is_some() {
        return Err(format!("CHARACTERISTIC {item_name} already exists."));
    }
    Ok(item_name)
}

fn make_unique_instance_name(
    module: &Module,
    sym_map: &HashMap<String, ItemType>,
    instance_sym: &str,
    name_map: &HashMap<String, ItemType>,
) -> Result<String, String> {
    // ideally the item name is the symbol name.
    // if the symbol is a demangled c++ symbol, then it might contain a "::", e.g. namespace::variable
    let cleaned_sym = instance_sym.replace("::", "__");

    // If an object of a different type already has this name, add the prefix "INSTANCE."
    let item_name = match sym_map.get(&cleaned_sym) {
        Some(ItemType::Instance(idx)) => {
            return Err(format!(
                "INSTANCE {} already references symbol {}.",
                module.instance[*idx].name, instance_sym
            ))
        }
        Some(
            ItemType::Measurement(_)
            | ItemType::Characteristic(_)
            | ItemType::Blob
            | ItemType::AxisPts,
        ) => {
            format!("INSTANCE.{cleaned_sym}")
        }
        None => cleaned_sym,
    };
    // fail if the name still isn't unique
    if name_map.get(&item_name).is_some() {
        return Err(format!("INSTANCE {item_name} already exists."));
    }
    Ok(item_name)
}

fn build_maps(module: &&mut Module) -> (HashMap<String, ItemType>, HashMap<String, ItemType>) {
    let mut name_map = HashMap::<String, ItemType>::new();
    let mut sym_map = HashMap::<String, ItemType>::new();
    for (idx, chr) in module.characteristic.iter().enumerate() {
        name_map.insert(chr.name.clone(), ItemType::Characteristic(idx));
        if let Some(sym_link) = &chr.symbol_link {
            sym_map.insert(sym_link.symbol_name.clone(), ItemType::Characteristic(idx));
        }
    }
    for (idx, meas) in module.measurement.iter().enumerate() {
        name_map.insert(meas.name.clone(), ItemType::Measurement(idx));
        if let Some(sym_link) = &meas.symbol_link {
            sym_map.insert(sym_link.symbol_name.clone(), ItemType::Measurement(idx));
        }
    }
    for (idx, inst) in module.instance.iter().enumerate() {
        name_map.insert(inst.name.clone(), ItemType::Instance(idx));
        if let Some(sym_link) = &inst.symbol_link {
            sym_map.insert(sym_link.symbol_name.clone(), ItemType::Instance(idx));
        }
    }
    for blob in &module.blob {
        name_map.insert(blob.name.clone(), ItemType::Blob);
        if let Some(sym_link) = &blob.symbol_link {
            sym_map.insert(sym_link.symbol_name.clone(), ItemType::Blob);
        }
    }
    for axis_pts in &module.axis_pts {
        name_map.insert(axis_pts.name.clone(), ItemType::AxisPts);
        if let Some(sym_link) = &axis_pts.symbol_link {
            sym_map.insert(sym_link.symbol_name.clone(), ItemType::AxisPts);
        }
    }

    (name_map, sym_map)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_many<'param>(
    a2l_file: &mut A2lFile,
    debugdata: &DebugData,
    measurement_ranges: &'param [(u64, u64)],
    characteristic_ranges: &'param [(u64, u64)],
    measurement_regexes: Vec<&str>,
    characteristic_regexes: Vec<&str>,
    target_group: Option<&str>,
    log_msgs: &mut Vec<String>,
    enable_structures: bool,
) {
    let file_version = crate::A2lVersion::from(&*a2l_file);
    let use_new_arrays = file_version >= A2lVersion::V1_7_0;
    let module = &mut a2l_file.project.module[0];
    let (name_map, sym_map) = build_maps(&module);
    let mut isupp = InsertSupport {
        module,
        debug_data: debugdata,
        compiled_meas_re: Vec::new(),
        compiled_char_re: Vec::new(),
        measurement_ranges,
        characteristic_ranges,
        name_map,
        sym_map,
        characteristic_list: Vec::new(),
        measurement_list: Vec::new(),
        meas_count: 0u32,
        chara_count: 0u32,
        instance_count: 0u32,
        version: file_version,
        create_typedef: Vec::new(),
    };
    // compile the regular expressions
    for expr in measurement_regexes {
        match Regex::new(expr) {
            Ok(compiled_re) => isupp.compiled_meas_re.push(compiled_re),
            Err(error) => println!("Invalid regex \"{expr}\": {error}"),
        }
    }
    for expr in characteristic_regexes {
        match Regex::new(expr) {
            Ok(compiled_re) => isupp.compiled_char_re.push(compiled_re),
            Err(error) => println!("Invalid regex \"{expr}\": {error}"),
        }
    }

    let mut debugdata_iter = debugdata.iter(use_new_arrays);
    let mut current_item = debugdata_iter.next();
    while let Some(sym_info) = current_item {
        let mut skip_children = false;
        match &sym_info.typeinfo.datatype {
            DwarfDataType::TypeRef(_, _) | DwarfDataType::FuncPtr(_) => {}
            DwarfDataType::Other(_)
            | DwarfDataType::Pointer(_, _)
            | DwarfDataType::Struct { .. }
            | DwarfDataType::Class { .. }
            | DwarfDataType::Union { .. } => {
                if enable_structures && check_and_insert_instance(&mut isupp, &sym_info, log_msgs) {
                    skip_children = true;
                }
            }
            DwarfDataType::Array { arraytype, .. } => {
                if is_simple_type(arraytype) {
                    if check_and_insert_simple_type(&mut isupp, &sym_info, log_msgs) {
                        skip_children = true;
                    }
                } else if enable_structures
                    && check_and_insert_instance(&mut isupp, &sym_info, log_msgs)
                {
                    skip_children = true;
                }
            }
            DwarfDataType::Enum { .. }
            | DwarfDataType::Float
            | DwarfDataType::Double
            | DwarfDataType::Sint8
            | DwarfDataType::Sint16
            | DwarfDataType::Sint32
            | DwarfDataType::Sint64
            | DwarfDataType::Uint8
            | DwarfDataType::Uint16
            | DwarfDataType::Uint32
            | DwarfDataType::Uint64
            | DwarfDataType::Bitfield { .. } => {
                check_and_insert_simple_type(&mut isupp, &sym_info, log_msgs);
                skip_children = true;
            }
        }

        if skip_children {
            current_item = debugdata_iter.next_sibling();
        } else {
            current_item = debugdata_iter.next();
        }
    }

    if let Some(group_name) = target_group {
        create_or_update_group(
            isupp.module,
            group_name,
            isupp.characteristic_list,
            isupp.measurement_list,
        );
    }

    if enable_structures && isupp.instance_count > 0 {
        update::typedef::create_new_typedefs(
            isupp.module,
            isupp.debug_data,
            log_msgs,
            &isupp.create_typedef,
        );
    }

    if isupp.meas_count > 0 {
        log_msgs.push(format!("Inserted {} MEASUREMENTs", isupp.meas_count));
    }
    if isupp.chara_count > 0 {
        log_msgs.push(format!("Inserted {} CHARACTERISTICs", isupp.chara_count));
    }
}

fn is_simple_type(typeinfo: &TypeInfo) -> bool {
    matches!(
        &typeinfo.datatype,
        DwarfDataType::Enum { .. }
            | DwarfDataType::Float
            | DwarfDataType::Double
            | DwarfDataType::Sint8
            | DwarfDataType::Sint16
            | DwarfDataType::Sint32
            | DwarfDataType::Sint64
            | DwarfDataType::Uint8
            | DwarfDataType::Uint16
            | DwarfDataType::Uint32
            | DwarfDataType::Uint64
    )
}

fn check_and_insert_simple_type(
    isupp: &mut InsertSupport,
    sym_info: &SymbolInfo,
    log_msgs: &mut Vec<String>,
) -> bool {
    let mut any_inserted = false;

    // insert if the address is inside a given range, or if a regex matches the symbol name
    if is_insert_requested(
        sym_info.address,
        &sym_info.name,
        isupp.measurement_ranges,
        &isupp.compiled_meas_re,
    ) {
        match insert_measurement_sym(
            isupp.module,
            isupp.debug_data,
            sym_info,
            &isupp.name_map,
            &isupp.sym_map,
            isupp.version,
        ) {
            Ok(measurement_name) => {
                log_msgs.push(format!(
                    "Inserted MEASUREMENT {measurement_name} (0x{:08x})",
                    sym_info.address
                ));
                isupp.measurement_list.push(measurement_name.clone());
                isupp.meas_count += 1;

                // update mappings to prevent the creation of duplicates
                let it = ItemType::Measurement(isupp.module.measurement.len() - 1);
                isupp.name_map.insert(measurement_name, it);
                isupp.sym_map.insert(sym_info.name.clone(), it);

                any_inserted = true;
            }
            Err(errmsg) => {
                log_msgs.push(format!("Skipped: {errmsg}"));
            }
        }
    }

    // insert if the address is inside a given range, or if a regex matches the symbol name
    if is_insert_requested(
        sym_info.address,
        &sym_info.name,
        isupp.characteristic_ranges,
        &isupp.compiled_char_re,
    ) {
        match insert_characteristic_sym(
            isupp.module,
            isupp.debug_data,
            &sym_info.name,
            sym_info,
            &isupp.name_map,
            &isupp.sym_map,
            isupp.version,
            None,
            None,
        ) {
            Ok(characteristic_name) => {
                log_msgs.push(format!(
                    "Inserted CHARACTERISTIC {characteristic_name} (0x{:08x})",
                    sym_info.address
                ));
                isupp.characteristic_list.push(characteristic_name.clone());
                isupp.chara_count += 1;

                // update mappings to prevent the creation of duplicates
                let it = ItemType::Characteristic(isupp.module.characteristic.len() - 1);
                isupp.name_map.insert(characteristic_name, it);
                isupp.sym_map.insert(sym_info.name.clone(), it);

                any_inserted = true;
            }
            Err(errmsg) => {
                log_msgs.push(format!("Skipped: {errmsg}"));
            }
        }
    }

    any_inserted
}

fn check_and_insert_instance<'dbg>(
    isupp: &mut InsertSupport<'_, 'dbg, '_>,
    sym_info: &SymbolInfo<'dbg>,
    log_msgs: &mut Vec<String>,
) -> bool {
    let mut any_inserted = false;

    // insert if the address is inside a given range, or if a regex matches the symbol name
    if is_insert_requested(
        sym_info.address,
        &sym_info.name,
        isupp.measurement_ranges,
        &isupp.compiled_meas_re,
    ) {
        match insert_instance_sym(
            isupp.module,
            isupp.debug_data,
            &sym_info.name,
            sym_info,
            &isupp.name_map,
            &isupp.sym_map,
            false,
        ) {
            Ok((instance_name, typedef_typeinfo)) => {
                log_msgs.push(format!(
                    "Inserted INSTANCE {instance_name} for measurement (0x{:08x})",
                    sym_info.address
                ));
                isupp.measurement_list.push(instance_name.clone());
                isupp.instance_count += 1;

                // update mappings to prevent the creation of duplicates
                let it = ItemType::Instance(isupp.module.instance.len() - 1);
                isupp.name_map.insert(instance_name, it);
                isupp.sym_map.insert(sym_info.name.clone(), it);

                isupp
                    .create_typedef
                    .push((typedef_typeinfo, isupp.module.instance.len() - 1));
                any_inserted = true;
            }
            Err(errmsg) => {
                log_msgs.push(format!("Skipped: {errmsg}"));
            }
        }
    }

    // insert if the address is inside a given range, or if a regex matches the symbol name
    if is_insert_requested(
        sym_info.address,
        &sym_info.name,
        isupp.characteristic_ranges,
        &isupp.compiled_char_re,
    ) {
        match insert_instance_sym(
            isupp.module,
            isupp.debug_data,
            &sym_info.name,
            sym_info,
            &isupp.name_map,
            &isupp.sym_map,
            true,
        ) {
            Ok((instance_name, typedef_typeinfo)) => {
                log_msgs.push(format!(
                    "Inserted INSTANCE {instance_name} for calibration (0x{:08x})",
                    sym_info.address
                ));
                isupp.measurement_list.push(instance_name.clone());
                isupp.instance_count += 1;

                // update mappings to prevent the creation of duplicates
                let it = ItemType::Instance(isupp.module.instance.len() - 1);
                isupp.name_map.insert(instance_name, it);
                isupp.sym_map.insert(sym_info.name.clone(), it);

                isupp
                    .create_typedef
                    .push((typedef_typeinfo, isupp.module.instance.len() - 1));
                any_inserted = true;
            }
            Err(errmsg) => {
                log_msgs.push(format!("Skipped: {errmsg}"));
            }
        }
    }

    any_inserted
}

fn is_insert_requested(
    address: u64,
    symbol_name: &str,
    addr_ranges: &[(u64, u64)],
    name_regexes: &[Regex],
) -> bool {
    // insert the symbol if its address is within any of the given ranges
    addr_ranges
        .iter()
        .any(|(lower, upper)| *lower <= address && address < *upper)
        // alternatively insert the symbol if its name is matched by any regex
        || name_regexes
        .iter()
        .any(|re| re.is_match(symbol_name))
}

fn create_or_update_group(
    module: &mut Module,
    group_name: &str,
    characteristic_list: Vec<String>,
    measurement_list: Vec<String>,
) {
    // try to find an existing group with the given name
    let existing_group = module.group.iter_mut().find(|grp| grp.name == group_name);

    let group: &mut Group = if let Some(grp) = existing_group {
        grp
    } else {
        // create a new group
        let mut group = Group::new(group_name.to_string(), String::new());
        // the group is not a sub-group of some other group, so it gets the ROOT attribute
        group.root = Some(Root::new());
        module.group.push(group);
        let len = module.group.len();
        &mut module.group[len - 1]
    };

    // add all characteristics to the REF_CHARACTERISTIC block in the group
    if !characteristic_list.is_empty() {
        if group.ref_characteristic.is_none() {
            group.ref_characteristic = Some(RefCharacteristic::new());
        }
        if let Some(ref_characteristic) = &mut group.ref_characteristic {
            for new_characteristic in characteristic_list {
                ref_characteristic.identifier_list.push(new_characteristic);
            }
        }
    }

    // add all measurements to the REF_MEASUREMENT block in the group
    if !measurement_list.is_empty() {
        if group.ref_measurement.is_none() {
            group.ref_measurement = Some(RefMeasurement::new());
        }
        if let Some(ref_measurement) = &mut group.ref_measurement {
            for new_measurement in measurement_list {
                ref_measurement.identifier_list.push(new_measurement);
            }
        }
    }
}

fn insert_instance_sym<'dbg>(
    module: &mut Module,
    debug_data: &'dbg DebugData,
    instance_sym: &str,
    sym_info: &SymbolInfo<'dbg>,
    name_map: &HashMap<String, ItemType>,
    sym_map: &HashMap<String, ItemType>,
    is_calib: bool,
) -> Result<(String, &'dbg TypeInfo), String> {
    if !matches!(&sym_info.typeinfo.datatype, DwarfDataType::FuncPtr(_)) {
        // Abort if a INSTANCE for this symbol already exists. Warn if any other reference to the symbol exists
        let item_name = make_unique_instance_name(module, sym_map, &sym_info.name, name_map)?;

        // use "magic" names to signal to the typedef creation code which kind of typedef should be created for this INSTANCE
        let typdef_name = if is_calib {
            update::typedef::FLAG_CREATE_CALIB.to_string()
        } else {
            update::typedef::FLAG_CREATE_MEAS.to_string()
        };

        let mut new_instance_sym = Instance::new(
            item_name.clone(),
            format!("instance for symbol {}", sym_info.name),
            typdef_name,
            sym_info.address as u32,
        );

        // create a SYMBOL_LINK
        let symbol_link_text = make_symbol_link_string(sym_info, debug_data);
        new_instance_sym.symbol_link = Some(SymbolLink::new(symbol_link_text, 0));

        set_address_type(&mut new_instance_sym.address_type, sym_info.typeinfo);
        let typeinfo = sym_info
            .typeinfo
            .get_pointer(&debug_data.types)
            .map_or(sym_info.typeinfo, |(_, t)| t);

        set_matrix_dim(&mut new_instance_sym.matrix_dim, typeinfo, true);
        let typeinfo = typeinfo.get_arraytype().unwrap_or(typeinfo);

        // set the eddress of the new instance to be witten as hex
        new_instance_sym.get_layout_mut().item_location.3 = (0, true);

        module.instance.push(new_instance_sym);

        Ok((item_name, typeinfo))
    } else {
        Err(format!(
            "Cannot create an INSTANCE for {instance_sym} with unsuitable type {}",
            sym_info.typeinfo
        ))
    }
}
