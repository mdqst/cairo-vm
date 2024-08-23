use std::path::PathBuf;

use crate::{
    hint_processor::hint_processor_definition::HintProcessor,
    types::{builtin_name::BuiltinName, layout_name::LayoutName, program::Program},
    vm::{
        errors::{
            cairo_run_errors::CairoRunError, runner_errors::RunnerError, vm_exception::VmException,
        },
        runners::{cairo_pie::CairoPie, cairo_runner::CairoRunner},
        security::verify_secure_runner,
    },
};

use crate::Felt252;
use bincode::enc::write::Writer;

use thiserror_no_std::Error;

use crate::types::exec_scope::ExecutionScopes;
#[cfg(feature = "test_utils")]
use arbitrary::{self, Arbitrary};

#[cfg_attr(feature = "test_utils", derive(Arbitrary))]
pub struct CairoRunConfig<'a> {
    #[cfg_attr(feature = "test_utils", arbitrary(value = "main"))]
    pub entrypoint: &'a str,
    pub trace_enabled: bool,
    pub relocate_mem: bool,
    pub layout: LayoutName,
    pub cairo_layout_params_file: Option<PathBuf>,
    pub proof_mode: bool,
    pub secure_run: Option<bool>,
    pub disable_trace_padding: bool,
    pub allow_missing_builtins: Option<bool>,
}

impl<'a> Default for CairoRunConfig<'a> {
    fn default() -> Self {
        CairoRunConfig {
            entrypoint: "main",
            trace_enabled: false,
            relocate_mem: false,
            layout: LayoutName::plain,
            proof_mode: false,
            secure_run: None,
            disable_trace_padding: false,
            allow_missing_builtins: None,
            cairo_layout_params_file: None,
        }
    }
}

/// Runs a program with a customized execution scope.
pub fn cairo_run_program_with_initial_scope(
    program: &Program,
    cairo_run_config: &CairoRunConfig,
    hint_processor: &mut dyn HintProcessor,
    exec_scopes: ExecutionScopes,
) -> Result<CairoRunner, CairoRunError> {
    let secure_run = cairo_run_config
        .secure_run
        .unwrap_or(!cairo_run_config.proof_mode);

    let allow_missing_builtins = cairo_run_config
        .allow_missing_builtins
        .unwrap_or(cairo_run_config.proof_mode);

    let mut cairo_runner = CairoRunner::new(
        program,
        cairo_run_config.layout,
        cairo_run_config.cairo_layout_params_file.clone(),
        cairo_run_config.proof_mode,
        cairo_run_config.trace_enabled,
    )?;

    cairo_runner.exec_scopes = exec_scopes;

    let end = cairo_runner.initialize(allow_missing_builtins)?;
    // check step calculation

    cairo_runner
        .run_until_pc(end, hint_processor)
        .map_err(|err| VmException::from_vm_error(&cairo_runner, err))?;

    if cairo_run_config.proof_mode {
        cairo_runner.run_for_steps(1, hint_processor)?;
    }
    cairo_runner.end_run(
        cairo_run_config.disable_trace_padding,
        false,
        hint_processor,
    )?;

    cairo_runner.vm.verify_auto_deductions()?;
    cairo_runner.read_return_values(allow_missing_builtins)?;
    if cairo_run_config.proof_mode {
        cairo_runner.finalize_segments()?;
    }
    if secure_run {
        verify_secure_runner(&cairo_runner, true, None)?;
    }
    cairo_runner.relocate(cairo_run_config.relocate_mem)?;

    Ok(cairo_runner)
}

pub fn cairo_run_program(
    program: &Program,
    cairo_run_config: &CairoRunConfig,
    hint_processor: &mut dyn HintProcessor,
) -> Result<CairoRunner, CairoRunError> {
    cairo_run_program_with_initial_scope(
        program,
        cairo_run_config,
        hint_processor,
        ExecutionScopes::new(),
    )
}

pub fn cairo_run(
    program_content: &[u8],
    cairo_run_config: &CairoRunConfig,
    hint_processor: &mut dyn HintProcessor,
) -> Result<CairoRunner, CairoRunError> {
    let program = Program::from_bytes(program_content, Some(cairo_run_config.entrypoint))?;

    cairo_run_program(&program, cairo_run_config, hint_processor)
}
/// Runs a Cairo PIE generated by a previous cairo execution
/// To generate a cairo pie use the runner's method `get_cairo_pie`
/// Note: Cairo PIEs cannot be ran in proof_mode
/// WARNING: As the RunResources are part of the HintProcessor trait, the caller should make sure that
/// the number of steps in the `RunResources` matches that of the `ExecutionResources` in the `CairoPie`.
/// An error will be returned if this doesn't hold.
pub fn cairo_run_pie(
    pie: &CairoPie,
    cairo_run_config: &CairoRunConfig,
    hint_processor: &mut dyn HintProcessor,
) -> Result<CairoRunner, CairoRunError> {
    if cairo_run_config.proof_mode {
        return Err(RunnerError::CairoPieProofMode.into());
    }
    if !hint_processor
        .get_n_steps()
        .is_some_and(|steps| steps == pie.execution_resources.n_steps)
    {
        return Err(RunnerError::PieNStepsVsRunResourcesNStepsMismatch.into());
    }
    pie.run_validity_checks()?;
    let secure_run = cairo_run_config.secure_run.unwrap_or(true);

    let allow_missing_builtins = cairo_run_config.allow_missing_builtins.unwrap_or_default();

    let program = Program::from_stripped_program(&pie.metadata.program);
    let mut cairo_runner = CairoRunner::new(
        &program,
        cairo_run_config.layout,
        cairo_run_config.cairo_layout_params_file.clone(),
        false,
        cairo_run_config.trace_enabled,
    )?;

    let end = cairo_runner.initialize(allow_missing_builtins)?;
    cairo_runner.vm.finalize_segments_by_cairo_pie(pie);
    // Load builtin additional data
    for (name, data) in pie.additional_data.0.iter() {
        // Data is not trusted in secure_run, therefore we skip extending the hash builtin's data
        if matches!(name, BuiltinName::pedersen) && secure_run {
            continue;
        }
        if let Some(builtin) = cairo_runner
            .vm
            .builtin_runners
            .iter_mut()
            .find(|b| b.name() == *name)
        {
            builtin.extend_additional_data(data)?;
        }
    }
    // Load previous execution memory
    let n_extra_segments = pie.metadata.extra_segments.len();
    cairo_runner
        .vm
        .segments
        .load_pie_memory(&pie.memory, n_extra_segments)?;

    cairo_runner
        .run_until_pc(end, hint_processor)
        .map_err(|err| VmException::from_vm_error(&cairo_runner, err))?;

    cairo_runner.end_run(
        cairo_run_config.disable_trace_padding,
        false,
        hint_processor,
    )?;

    cairo_runner.vm.verify_auto_deductions()?;
    cairo_runner.read_return_values(allow_missing_builtins)?;

    if secure_run {
        verify_secure_runner(&cairo_runner, true, None)?;
        // Check that the Cairo PIE produced by this run is compatible with the Cairo PIE received
        cairo_runner.get_cairo_pie()?.check_pie_compatibility(pie)?;
    }
    cairo_runner.relocate(cairo_run_config.relocate_mem)?;

    Ok(cairo_runner)
}

#[cfg(feature = "test_utils")]
pub fn cairo_run_fuzzed_program(
    program: Program,
    cairo_run_config: &CairoRunConfig,
    hint_processor: &mut dyn HintProcessor,
    steps_limit: usize,
) -> Result<CairoRunner, CairoRunError> {
    use crate::vm::errors::vm_errors::VirtualMachineError;

    let secure_run = cairo_run_config
        .secure_run
        .unwrap_or(!cairo_run_config.proof_mode);

    let allow_missing_builtins = cairo_run_config
        .allow_missing_builtins
        .unwrap_or(cairo_run_config.proof_mode);

    let mut cairo_runner = CairoRunner::new(
        &program,
        cairo_run_config.layout,
        cairo_run_config.cairo_layout_params_file.clone(),
        cairo_run_config.proof_mode,
        cairo_run_config.trace_enabled,
    )?;

    let _end = cairo_runner.initialize(allow_missing_builtins)?;

    let res = match cairo_runner.run_until_steps(steps_limit, hint_processor) {
        Err(VirtualMachineError::EndOfProgram(_remaining)) => Ok(()), // program ran OK but ended before steps limit
        res => res,
    };

    res.map_err(|err| VmException::from_vm_error(&cairo_runner, err))?;

    cairo_runner.end_run(false, false, hint_processor)?;

    cairo_runner.vm.verify_auto_deductions()?;
    cairo_runner.read_return_values(allow_missing_builtins)?;
    if cairo_run_config.proof_mode {
        cairo_runner.finalize_segments()?;
    }
    if secure_run {
        verify_secure_runner(&cairo_runner, true, None)?;
    }
    cairo_runner.relocate(cairo_run_config.relocate_mem)?;

    Ok(cairo_runner)
}

#[derive(Debug, Error)]
#[error("Failed to encode trace at position {0}, serialize error: {1}")]
pub struct EncodeTraceError(usize, bincode::error::EncodeError);

/// Writes the trace binary representation.
///
/// Bincode encodes to little endian by default and each trace entry is composed of
/// 3 usize values that are padded to always reach 64 bit size.
pub fn write_encoded_trace(
    relocated_trace: &[crate::vm::trace::trace_entry::RelocatedTraceEntry],
    dest: &mut impl Writer,
) -> Result<(), EncodeTraceError> {
    for (i, entry) in relocated_trace.iter().enumerate() {
        dest.write(&((entry.ap as u64).to_le_bytes()))
            .map_err(|e| EncodeTraceError(i, e))?;
        dest.write(&((entry.fp as u64).to_le_bytes()))
            .map_err(|e| EncodeTraceError(i, e))?;
        dest.write(&((entry.pc as u64).to_le_bytes()))
            .map_err(|e| EncodeTraceError(i, e))?;
    }

    Ok(())
}

/// Writes a binary representation of the relocated memory.
///
/// The memory pairs (address, value) are encoded and concatenated:
/// * address -> 8-byte encoded
/// * value -> 32-byte encoded
pub fn write_encoded_memory(
    relocated_memory: &[Option<Felt252>],
    dest: &mut impl Writer,
) -> Result<(), EncodeTraceError> {
    for (i, memory_cell) in relocated_memory.iter().enumerate() {
        match memory_cell {
            None => continue,
            Some(unwrapped_memory_cell) => {
                dest.write(&(i as u64).to_le_bytes())
                    .map_err(|e| EncodeTraceError(i, e))?;
                dest.write(&unwrapped_memory_cell.to_bytes_le())
                    .map_err(|e| EncodeTraceError(i, e))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::prelude::*;
    use crate::vm::runners::cairo_runner::RunResources;
    use crate::Felt252;
    use crate::{
        hint_processor::{
            builtin_hint_processor::builtin_hint_processor_definition::BuiltinHintProcessor,
            hint_processor_definition::HintProcessor,
        },
        utils::test_utils::*,
    };
    use bincode::enc::write::SliceWriter;

    use rstest::rstest;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::*;

    fn run_test_program(
        program_content: &[u8],
        hint_processor: &mut dyn HintProcessor,
    ) -> Result<CairoRunner, CairoRunError> {
        let program = Program::from_bytes(program_content, Some("main")).unwrap();
        let mut cairo_runner = cairo_runner!(program, LayoutName::all_cairo, false, true);
        let end = cairo_runner
            .initialize(false)
            .map_err(CairoRunError::Runner)?;

        assert!(cairo_runner.run_until_pc(end, hint_processor).is_ok());

        Ok(cairo_runner)
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn cairo_run_custom_entry_point() {
        let program = Program::from_bytes(
            include_bytes!("../../cairo_programs/not_main.json"),
            Some("not_main"),
        )
        .unwrap();
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let mut cairo_runner = cairo_runner!(program);

        let end = cairo_runner.initialize(false).unwrap();
        assert!(cairo_runner.run_until_pc(end, &mut hint_processor).is_ok());
        assert!(cairo_runner.relocate(true).is_ok());
        // `main` returns without doing nothing, but `not_main` sets `[ap]` to `1`
        // Memory location was found empirically and simply hardcoded
        assert_eq!(cairo_runner.relocated_memory[2], Some(Felt252::from(123)));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn cairo_run_with_no_data_program() {
        // a compiled program with no `data` key.
        // it should fail when the program is loaded.
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let no_data_program_path =
            include_bytes!("../../cairo_programs/manually_compiled/no_data_program.json");
        let cairo_run_config = CairoRunConfig::default();
        assert!(cairo_run(no_data_program_path, &cairo_run_config, &mut hint_processor,).is_err());
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn cairo_run_with_no_main_program() {
        // a compiled program with no main scope
        // it should fail when trying to run initialize_main_entrypoint.
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let no_main_program =
            include_bytes!("../../cairo_programs/manually_compiled/no_main_program.json");
        let cairo_run_config = CairoRunConfig::default();
        assert!(cairo_run(no_main_program, &cairo_run_config, &mut hint_processor,).is_err());
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn cairo_run_with_invalid_memory() {
        // the program invalid_memory.json has an invalid memory cell and errors when trying to
        // decode the instruction.
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let invalid_memory =
            include_bytes!("../../cairo_programs/manually_compiled/invalid_memory.json");
        let cairo_run_config = CairoRunConfig::default();
        assert!(cairo_run(invalid_memory, &cairo_run_config, &mut hint_processor,).is_err());
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn write_output_program() {
        let program_content = include_bytes!("../../cairo_programs/bitwise_output.json");
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let mut runner = run_test_program(program_content, &mut hint_processor)
            .expect("Couldn't initialize cairo runner");

        let mut output_buffer = String::new();
        runner.vm.write_output(&mut output_buffer).unwrap();
        assert_eq!(&output_buffer, "0\n");
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn write_binary_trace_file() {
        let program_content = include_bytes!("../../cairo_programs/struct.json");
        let expected_encoded_trace =
            include_bytes!("../../cairo_programs/trace_memory/cairo_trace_struct");

        // run test program until the end
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let mut cairo_runner = run_test_program(program_content, &mut hint_processor).unwrap();

        assert!(cairo_runner.relocate(false).is_ok());

        let trace_entries = cairo_runner.relocated_trace.unwrap();
        let mut buffer = [0; 24];
        let mut buff_writer = SliceWriter::new(&mut buffer);
        // write cairo_rs vm trace file
        write_encoded_trace(&trace_entries, &mut buff_writer).unwrap();

        // compare that the original cairo vm trace file and cairo_rs vm trace files are equal
        assert_eq!(buffer, *expected_encoded_trace);
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn write_binary_memory_file() {
        let program_content = include_bytes!("../../cairo_programs/struct.json");
        let expected_encoded_memory =
            include_bytes!("../../cairo_programs/trace_memory/cairo_memory_struct");

        // run test program until the end
        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let mut cairo_runner = run_test_program(program_content, &mut hint_processor).unwrap();

        // relocate memory so we can dump it to file
        assert!(cairo_runner.relocate(true).is_ok());

        let mut buffer = [0; 120];
        let mut buff_writer = SliceWriter::new(&mut buffer);
        // write cairo_rs vm memory file
        write_encoded_memory(&cairo_runner.relocated_memory, &mut buff_writer).unwrap();

        // compare that the original cairo vm memory file and cairo_rs vm memory files are equal
        assert_eq!(*expected_encoded_memory, buffer);
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn run_with_no_trace() {
        let program = Program::from_bytes(
            include_bytes!("../../cairo_programs/struct.json"),
            Some("main"),
        )
        .unwrap();

        let mut hint_processor = BuiltinHintProcessor::new_empty();
        let mut cairo_runner = cairo_runner!(program);
        let end = cairo_runner.initialize(false).unwrap();
        assert!(cairo_runner.run_until_pc(end, &mut hint_processor).is_ok());
        assert!(cairo_runner.relocate(false).is_ok());
        assert!(cairo_runner.relocated_trace.is_none());
    }

    #[rstest]
    #[case(include_bytes!("../../cairo_programs/fibonacci.json"))]
    #[case(include_bytes!("../../cairo_programs/integration.json"))]
    #[case(include_bytes!("../../cairo_programs/common_signature.json"))]
    #[case(include_bytes!("../../cairo_programs/relocate_segments.json"))]
    #[case(include_bytes!("../../cairo_programs/ec_op.json"))]
    #[case(include_bytes!("../../cairo_programs/bitwise_output.json"))]
    #[case(include_bytes!("../../cairo_programs/value_beyond_segment.json"))]
    fn get_and_run_cairo_pie(#[case] program_content: &[u8]) {
        let cairo_run_config = CairoRunConfig {
            layout: LayoutName::starknet_with_keccak,
            ..Default::default()
        };
        // First run program to get Cairo PIE
        let cairo_pie = {
            let runner = cairo_run(
                program_content,
                &cairo_run_config,
                &mut BuiltinHintProcessor::new_empty(),
            )
            .unwrap();
            runner.get_cairo_pie().unwrap()
        };
        let mut hint_processor = BuiltinHintProcessor::new(
            Default::default(),
            RunResources::new(cairo_pie.execution_resources.n_steps),
        );
        // Default config runs with secure_run, which checks that the Cairo PIE produced by this run is compatible with the one received
        assert!(cairo_run_pie(&cairo_pie, &cairo_run_config, &mut hint_processor).is_ok());
    }

    #[test]
    fn cairo_run_pie_n_steps_not_set() {
        // First run program to get Cairo PIE
        let cairo_pie = {
            let runner = cairo_run(
                include_bytes!("../../cairo_programs/fibonacci.json"),
                &CairoRunConfig::default(),
                &mut BuiltinHintProcessor::new_empty(),
            )
            .unwrap();
            runner.get_cairo_pie().unwrap()
        };
        // Run Cairo PIE
        let res = cairo_run_pie(
            &cairo_pie,
            &CairoRunConfig::default(),
            &mut BuiltinHintProcessor::new_empty(),
        );
        assert!(res.is_err_and(|err| matches!(
            err,
            CairoRunError::Runner(RunnerError::PieNStepsVsRunResourcesNStepsMismatch)
        )));
    }
}
