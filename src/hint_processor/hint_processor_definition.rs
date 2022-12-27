use crate::{
    serde::deserialize_program::{ApTracking, OffsetValue},
    types::{exec_scope::ExecutionScopes, instruction::Register},
    vm::errors::vm_errors::VirtualMachineError,
    vm::vm_core::VirtualMachine,
};
use felt::Felt;
use std::{any::Any, collections::HashMap};

pub trait HintProcessor {
    //Executes the hint which's data is provided by a dynamic structure previously created by compile_hint
    fn execute_hint(
        &mut self,
        //Proxy to VM, contains refrences to necessary data
        //+ MemoryProxy, which provides the necessary methods to manipulate memory
        vm: &mut VirtualMachine,
        //Proxy to ExecutionScopes, provides the necessary methods to manipulate the scopes and
        //access current scope variables
        exec_scopes: &mut ExecutionScopes,
        //Data structure that can be downcasted to the structure generated by compile_hint
        hint_data: &Box<dyn Any>,
        //Constant values extracted from the program specification.
        constants: &HashMap<String, Felt>,
    ) -> Result<(), VirtualMachineError>;

    //Transforms hint data outputed by the VM into whichever format will be later used by execute_hint
    fn compile_hint(
        &self,
        //Block of hint code as String
        hint_code: &str,
        //Ap Tracking Data corresponding to the Hint
        ap_tracking_data: &ApTracking,
        //Map from variable name to reference id number
        //(may contain other variables aside from those used by the hint)
        reference_ids: &HashMap<String, usize>,
        //List of all references (key corresponds to element of the previous dictionary)
        references: &HashMap<usize, HintReference>,
    ) -> Result<Box<dyn Any>, VirtualMachineError>;
}

#[derive(Debug, PartialEq, Clone)]
pub struct HintReference {
    pub offset1: OffsetValue,
    pub offset2: OffsetValue,
    pub dereference: bool,
    pub ap_tracking_data: Option<ApTracking>,
    pub cairo_type: Option<String>,
}

impl HintReference {
    pub fn new_simple(offset1: i32) -> Self {
        HintReference {
            offset1: OffsetValue::Reference(Register::FP, offset1, false),
            offset2: OffsetValue::Value(0),
            ap_tracking_data: None,
            dereference: true,
            cairo_type: None,
        }
    }

    pub fn new(offset1: i32, offset2: i32, inner_dereference: bool, dereference: bool) -> Self {
        HintReference {
            offset1: OffsetValue::Reference(Register::FP, offset1, inner_dereference),
            offset2: OffsetValue::Value(offset2),
            ap_tracking_data: None,
            dereference,
            cairo_type: None,
        }
    }
}