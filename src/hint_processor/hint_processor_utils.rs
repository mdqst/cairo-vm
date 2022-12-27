use crate::{
    serde::deserialize_program::{ApTracking, OffsetValue},
    types::{
        instruction::Register,
        relocatable::{MaybeRelocatable, Relocatable},
    },
    vm::{errors::vm_errors::VirtualMachineError, vm_core::VirtualMachine},
};
use std::borrow::Cow;

use super::hint_processor_definition::HintReference;
use felt::Felt;
use num_traits::ToPrimitive;

///Inserts value into the address of the given ids variable
pub fn insert_value_from_reference(
    value: impl Into<MaybeRelocatable>,
    vm: &mut VirtualMachine,
    hint_reference: &HintReference,
    ap_tracking: &ApTracking,
) -> Result<(), VirtualMachineError> {
    let var_addr = compute_addr_from_reference(hint_reference, vm, ap_tracking)?;
    vm.insert_value(&var_addr, value)
}

///Returns the Integer value stored in the given ids variable
pub fn get_integer_from_reference<'a>(
    vm: &'a VirtualMachine,
    hint_reference: &'a HintReference,
    ap_tracking: &ApTracking,
) -> Result<Cow<'a, Felt>, VirtualMachineError> {
    // if the reference register is none, this means it is an immediate value and we
    // should return that value.

    if let (OffsetValue::Immediate(int_1), _) = (&hint_reference.offset1, &hint_reference.offset2) {
        return Ok(Cow::Borrowed(int_1));
    }

    let var_addr = compute_addr_from_reference(hint_reference, vm, ap_tracking)?;
    vm.get_integer(&var_addr)
}

///Returns the Relocatable value stored in the given ids variable
pub fn get_ptr_from_reference(
    vm: &VirtualMachine,
    hint_reference: &HintReference,
    ap_tracking: &ApTracking,
) -> Result<Relocatable, VirtualMachineError> {
    let var_addr = compute_addr_from_reference(hint_reference, vm, ap_tracking)?;
    if hint_reference.dereference {
        Ok(vm.get_relocatable(&var_addr)?)
    } else {
        Ok(var_addr)
    }
}

///Computes the memory address of the ids variable indicated by the HintReference as a Relocatable
pub fn compute_addr_from_reference(
    //Reference data of the ids variable
    hint_reference: &HintReference,
    vm: &VirtualMachine,
    //ApTracking of the Hint itself
    hint_ap_tracking: &ApTracking,
) -> Result<Relocatable, VirtualMachineError> {
    let offset1 =
        if let OffsetValue::Reference(_register, _offset, _deref) = &hint_reference.offset1 {
            get_offset_value_reference(
                vm,
                hint_reference,
                hint_ap_tracking,
                &hint_reference.offset1,
            )?
            .get_relocatable()?
        } else {
            return Err(VirtualMachineError::NoRegisterInReference);
        };

    match &hint_reference.offset2 {
        OffsetValue::Reference(_register, _offset, _deref) => {
            // Cant add two relocatable values
            // So OffSet2 must be Bigint
            let value = get_offset_value_reference(
                vm,
                hint_reference,
                hint_ap_tracking,
                &hint_reference.offset2,
            )?;

            Ok(offset1
                + value
                    .get_int_ref()?
                    .to_usize()
                    .ok_or(VirtualMachineError::BigintToUsizeFail)?)
        }
        OffsetValue::Value(value) => Ok(offset1 + *value),
        _ => Err(VirtualMachineError::NoRegisterInReference),
    }
}

fn apply_ap_tracking_correction(
    ap: &Relocatable,
    ref_ap_tracking: &ApTracking,
    hint_ap_tracking: &ApTracking,
) -> Result<Relocatable, VirtualMachineError> {
    // check that both groups are the same
    if ref_ap_tracking.group != hint_ap_tracking.group {
        return Err(VirtualMachineError::InvalidTrackingGroup(
            ref_ap_tracking.group,
            hint_ap_tracking.group,
        ));
    }
    let ap_diff = hint_ap_tracking.offset - ref_ap_tracking.offset;
    ap.sub_usize(ap_diff)
}

//Tries to convert a Felt value to usize
pub fn felt_to_usize(felt: &Felt) -> Result<usize, VirtualMachineError> {
    felt.to_usize()
        .ok_or(VirtualMachineError::BigintToUsizeFail)
}

///Tries to convert a Felt value to u32
pub fn felt_to_u32(felt: &Felt) -> Result<u32, VirtualMachineError> {
    felt.to_u32().ok_or(VirtualMachineError::BigintToU32Fail)
}

fn get_offset_value_reference(
    vm: &VirtualMachine,
    hint_reference: &HintReference,
    hint_ap_tracking: &ApTracking,
    offset_value: &OffsetValue,
) -> Result<MaybeRelocatable, VirtualMachineError> {
    // let (register, offset , deref) = if let OffsetValue::Reference(register, offset ,deref ) = offset_value {
    //     (register, offset_value, deref)
    // } else {
    //      return Err(VirtualMachineError::FailedToGetIds);
    // };
    let (register, offset, deref) = match offset_value {
        OffsetValue::Reference(register, offset, deref) => (register, offset, deref),
        _ => return Err(VirtualMachineError::FailedToGetIds),
    };

    let base_addr = if register == &Register::FP {
        vm.get_fp()
    } else {
        let var_ap_trackig = hint_reference
            .ap_tracking_data
            .as_ref()
            .ok_or(VirtualMachineError::NoneApTrackingData)?;

        apply_ap_tracking_correction(&vm.get_ap(), var_ap_trackig, hint_ap_tracking)?
    };

    if offset.is_negative() && base_addr.offset < offset.abs() as usize {
        return Err(VirtualMachineError::FailedToGetIds);
    }

    if *deref {
        Ok(vm
            .get_maybe(&(base_addr + *offset))
            .map_err(|_| VirtualMachineError::FailedToGetIds)?
            .ok_or(VirtualMachineError::FailedToGetIds)?)
    } else {
        Ok((base_addr + *offset).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        relocatable,
        utils::test_utils::*,
        vm::{
            errors::memory_errors::MemoryError, vm_core::VirtualMachine, vm_memory::memory::Memory,
        },
    };
    use felt::NewFelt;

    #[test]
    fn get_integer_from_reference_with_immediate_value() {
        let mut vm = vm!();
        vm.memory = memory![((1, 0), 0)];
        let mut hint_ref = HintReference::new(0, 0, false, true);
        hint_ref.offset1 = OffsetValue::Immediate(Felt::new(2));

        assert_eq!(
            get_integer_from_reference(&vm, &hint_ref, &ApTracking::new())
                .expect("Unexpected get integer fail")
                .into_owned(),
            Felt::new(2)
        );
    }

    #[test]
    fn get_ptr_from_reference_short_path() {
        let mut vm = vm!();
        vm.memory = memory![((1, 0), (2, 0))];

        assert_eq!(
            get_ptr_from_reference(
                &vm,
                &HintReference::new(0, 0, false, false),
                &ApTracking::new()
            ),
            Ok(relocatable!(1, 0))
        );
    }

    #[test]
    fn get_ptr_from_reference_with_dereference() {
        let mut vm = vm!();
        vm.memory = memory![((1, 0), (3, 0))];

        assert_eq!(
            get_ptr_from_reference(
                &vm,
                &HintReference::new(0, 0, false, true),
                &ApTracking::new()
            ),
            Ok(relocatable!(3, 0))
        );
    }

    #[test]
    fn get_ptr_from_reference_with_dereference_and_imm() {
        let mut vm = vm!();
        vm.memory = memory![((1, 0), (4, 0))];
        let mut hint_ref = HintReference::new(0, 0, true, false);
        hint_ref.offset2 = OffsetValue::Value(2);

        assert_eq!(
            get_ptr_from_reference(&vm, &hint_ref, &ApTracking::new()),
            Ok(relocatable!(4, 2))
        );
    }

    #[test]
    fn compute_addr_from_reference_no_regiter_in_reference() {
        let mut vm = vm!();
        vm.memory = memory![((1, 0), (4, 0))];
        let mut hint_reference = HintReference::new(0, 0, false, false);
        hint_reference.offset1 = OffsetValue::Immediate(Felt::new(2_i32));

        assert_eq!(
            compute_addr_from_reference(&hint_reference, &vm, &ApTracking::new()),
            Err(VirtualMachineError::NoRegisterInReference)
        );
    }

    #[test]
    fn compute_addr_from_reference_failed_to_get_ids() {
        let mut vm = vm!();
        vm.memory = memory![((1, 0), 4)];
        // vm.run_context.fp = -1;
        let mut hint_reference = HintReference::new(0, 0, false, false);
        hint_reference.offset1 = OffsetValue::Reference(Register::FP, -1, true);

        assert_eq!(
            compute_addr_from_reference(&hint_reference, &vm, &ApTracking::new()),
            Err(VirtualMachineError::FailedToGetIds)
        );
    }

    #[test]
    fn tracking_correction_invalid_group() {
        let mut ref_ap_tracking = ApTracking::new();
        ref_ap_tracking.group = 1;
        let mut hint_ap_tracking = ApTracking::new();
        hint_ap_tracking.group = 2;

        assert_eq!(
            apply_ap_tracking_correction(&relocatable!(1, 0), &ref_ap_tracking, &hint_ap_tracking),
            Err(VirtualMachineError::InvalidTrackingGroup(1, 2))
        );
    }
}