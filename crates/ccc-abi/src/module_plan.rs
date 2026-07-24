use std::fmt::Write as _;

use ccc_ir::generic as gir;
use ccc_target::EffectiveCompilationConfig;
use sha2::{Digest as _, Sha256};

use crate::{
    AbiCarrier, AbiClass, AbiError, BoundaryPlan, BridgeArtifactPlan, BridgeEntryArtifactPlan,
    BridgeLocation, CallBridgeArtifactPlan, CallPlan, CallTarget, DefinitionPlan,
    F80SupportArtifactPlan, InlineAsmSupportArtifactPlan, LoweredSignaturePlan, ModuleAbiPlan,
    NativePurpose, PackagingPlan, PassingMode, SourceBinding, SourceLinkage, SourceVisibility,
    TlsAccessorArtifactPlan, VerifiedModuleAbiPlan, abi_config_key, hex, ir_shape_digest,
    plan_boundary_type, plan_fixed_call, plan_function_type, plan_unprototyped_call, plan_va_arg,
    plan_variadic_call, translation_unit_digest,
};

pub fn plan_module(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
) -> Result<ModuleAbiPlan, AbiError> {
    crate::validate_target(config)?;
    let required_symbols = module.source_symbol_requirements();
    if !config.target.abi.supports_tls_codegen()
        && module.globals.iter().any(|global| {
            required_symbols.objects.contains(&global.id)
                && (global.duration == ccc_sema::generic::StorageDuration::Thread
                    || global.emission.tls.is_some())
        })
    {
        return Err(AbiError::new(
            "CCC3522",
            format!(
                "thread-local storage has no enabled object and link contract for target ABI `{}`",
                config.target.abi.name()
            ),
        ));
    }
    let config_key = abi_config_key(config)?;
    let ir_shape_digest = ir_shape_digest(module, &config_key)?;
    let translation_unit_digest = translation_unit_digest(module, &config_key, ir_shape_digest);
    let mut definitions = std::collections::BTreeMap::new();
    let mut calls = std::collections::BTreeMap::new();
    let mut va_args = std::collections::BTreeMap::new();

    for function in &module.functions {
        if function.entry.is_some() {
            let boundary = plan_boundary_type(&module.types, function.signature, config)
                .map_err(|error| error.with_span_if_none(function.span))?;
            definitions.insert(
                function.id,
                DefinitionPlan {
                    source_signature: function.signature,
                    lowered_signature: lowered_signature(&boundary),
                    source_location: function.span,
                    boundary,
                },
            );
        }
        for block in &function.blocks {
            for instruction in &block.instructions {
                let (signature, arguments, variadic_boundary, target) = match &instruction.kind {
                    gir::FullInstructionKind::DirectCall {
                        function: callee,
                        signature,
                        arguments,
                        variadic_boundary,
                        ..
                    } => (
                        *signature,
                        arguments.as_slice(),
                        *variadic_boundary,
                        CallTarget::Direct(*callee),
                    ),
                    gir::FullInstructionKind::IndirectCall {
                        callee,
                        signature,
                        arguments,
                        variadic_boundary,
                        ..
                    } => (
                        *signature,
                        arguments.as_slice(),
                        *variadic_boundary,
                        CallTarget::Indirect(*callee),
                    ),
                    gir::FullInstructionKind::VaArg { requested, .. } => {
                        let plan = plan_va_arg(&module.types, requested.ty, config)
                            .map_err(|error| error.with_span_if_none(instruction.span))?;
                        va_args.insert((function.id, instruction.id), plan);
                        continue;
                    }
                    _ => continue,
                };
                let actual_types = arguments
                    .iter()
                    .map(|argument| {
                        function
                            .value_types
                            .get(argument.0 as usize)
                            .copied()
                            .ok_or_else(|| {
                                AbiError::new(
                                    "CCC3515",
                                    format!(
                                        "call instruction {} references unknown value {}",
                                        instruction.id.0, argument.0
                                    ),
                                )
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let signature_data =
                    module.types.function_signature(signature).ok_or_else(|| {
                        AbiError::new(
                            "CCC3505",
                            format!(
                                "call instruction {} carries non-function type `{}`",
                                instruction.id.0,
                                module.types.display(signature)
                            ),
                        )
                    })?;
                let boundary = if matches!(
                    signature_data.parameters,
                    ccc_types::FunctionParameters::Unspecified
                ) {
                    if variadic_boundary != 0 {
                        return Err(AbiError::new(
                            "CCC3512",
                            format!(
                                "unprototyped call instruction {} carries nonzero fixed boundary {variadic_boundary}",
                                instruction.id.0
                            ),
                        )
                        .with_span_if_none(instruction.span));
                    }
                    BoundaryPlan::Bridge(
                        plan_unprototyped_call(&module.types, signature, &actual_types, config)
                            .map_err(|error| error.with_span_if_none(instruction.span))?,
                    )
                } else if signature_data.variadic {
                    BoundaryPlan::Bridge(
                        plan_variadic_call(
                            &module.types,
                            signature,
                            &actual_types,
                            variadic_boundary,
                            config,
                        )
                        .map_err(|error| error.with_span_if_none(instruction.span))?,
                    )
                } else if function_signature_requires_bridge(&module.types, signature, config) {
                    BoundaryPlan::Bridge(
                        plan_fixed_call(&module.types, signature, &actual_types, config)
                            .map_err(|error| error.with_span_if_none(instruction.span))?,
                    )
                } else {
                    let plan = plan_function_type(&module.types, signature, config)
                        .map_err(|error| error.with_span_if_none(instruction.span))?;
                    if plan.parameters.len() != arguments.len()
                        || variadic_boundary != arguments.len()
                    {
                        return Err(AbiError::new(
                            "CCC3512",
                            format!(
                                "call instruction {} expects {} arguments but carries {} with boundary {variadic_boundary}",
                                instruction.id.0,
                                plan.parameters.len(),
                                arguments.len()
                            ),
                        )
                        .with_span_if_none(instruction.span));
                    }
                    BoundaryPlan::Native(plan)
                };
                let call_plan = CallPlan {
                    source_signature: signature,
                    lowered_signature: lowered_signature(&boundary),
                    target,
                    promoted_actual_types: actual_types,
                    fixed_boundary: variadic_boundary,
                    source_location: instruction.span,
                    boundary,
                };
                if calls
                    .insert((function.id, instruction.id), call_plan)
                    .is_some()
                {
                    return Err(AbiError::new(
                        "CCC3515",
                        format!(
                            "duplicate ABI call identity ({}, {})",
                            function.id.0, instruction.id.0
                        ),
                    ));
                }
            }
        }
    }
    let artifacts = plan_artifacts(
        module,
        &definitions,
        &calls,
        &required_symbols,
        translation_unit_digest,
        config.target.abi,
    )?;
    Ok(ModuleAbiPlan {
        config_key,
        ir_shape_digest,
        translation_unit_digest,
        definitions,
        calls,
        va_args,
        artifacts,
    })
}

fn function_signature_requires_bridge(
    types: &ccc_types::TypeStore,
    signature: ccc_types::TypeId,
    config: &EffectiveCompilationConfig,
) -> bool {
    fn contains(
        types: &ccc_types::TypeStore,
        ty: ccc_types::TypeId,
        config: &EffectiveCompilationConfig,
        active: &mut Vec<ccc_types::TypeId>,
    ) -> bool {
        match types.builtin_type(ty) {
            Some(ccc_types::BuiltinType::Int128 | ccc_types::BuiltinType::UnsignedInt128) => {
                return true;
            }
            Some(ccc_types::BuiltinType::LongDouble) => {
                return config.target.data_layout.long_double_format
                    == ccc_target::LongDoubleFormat::X87Extended;
            }
            _ => {}
        }
        if active.contains(&ty) {
            return false;
        }
        active.push(ty);
        let result = match types.try_kind(ty) {
            Some(ccc_types::TypeKind::Array(array)) => {
                contains(types, array.element.ty, config, active)
            }
            Some(ccc_types::TypeKind::Record(id)) => types
                .record(*id)
                .and_then(|record| record.fields.as_ref())
                .is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| contains(types, field.ty.ty, config, active))
                }),
            _ => false,
        };
        active.pop();
        result
    }

    let Some(signature) = types.function_signature(signature) else {
        return false;
    };
    contains(types, signature.result.ty, config, &mut Vec::new())
        || match signature.parameters {
            ccc_types::FunctionParameters::Prototype(parameters) => parameters
                .iter()
                .any(|parameter| contains(types, parameter.ty, config, &mut Vec::new())),
            ccc_types::FunctionParameters::Unspecified => false,
        }
}

fn plan_artifacts(
    module: &gir::FullModule,
    definitions: &std::collections::BTreeMap<ccc_sema::generic::FullFunctionId, DefinitionPlan>,
    calls: &std::collections::BTreeMap<
        (ccc_sema::generic::FullFunctionId, gir::InstructionId),
        CallPlan,
    >,
    required_symbols: &gir::SourceSymbolRequirements,
    translation_unit_digest: crate::TranslationUnitDigest,
    abi_identity: ccc_target::AbiIdentity,
) -> Result<BridgeArtifactPlan, AbiError> {
    let inline_helpers = module.required_native_inline_asm_helpers();
    if !inline_helpers.is_empty() && abi_identity != ccc_target::AbiIdentity::SysvAmd64Lp64 {
        return Err(AbiError::new(
            "CCC3515",
            "x86 inline-assembly helpers require the System V AMD64 ABI",
        ));
    }
    let inline_asm_support = (!inline_helpers.is_empty()).then(|| InlineAsmSupportArtifactPlan {
        cpuid_symbol: inline_helpers
            .contains(&gir::NativeInlineAsmHelper::X86Cpuid)
            .then(|| {
                generated_symbol_for(
                    translation_unit_digest,
                    "support_cpuid",
                    ccc_sema::generic::FullFunctionId(u32::MAX - 2),
                    None,
                )
            }),
        rdtsc_symbol: inline_helpers
            .contains(&gir::NativeInlineAsmHelper::X86Rdtsc)
            .then(|| {
                generated_symbol_for(
                    translation_unit_digest,
                    "support_rdtsc",
                    ccc_sema::generic::FullFunctionId(u32::MAX - 3),
                    None,
                )
            }),
    });
    let f80_support = (abi_identity == ccc_target::AbiIdentity::SysvAmd64Lp64
        && module.functions.iter().any(|function| {
            function.value_types.iter().any(|ty| {
                module.types.builtin_type(*ty) == Some(ccc_types::BuiltinType::LongDouble)
            })
        }))
    .then(|| F80SupportArtifactPlan {
        helper_symbol: generated_symbol_for(
            translation_unit_digest,
            "support_f80",
            ccc_sema::generic::FullFunctionId(u32::MAX - 1),
            None,
        ),
    });
    let call_sites = calls
        .iter()
        .filter_map(|(site, plan)| {
            matches!(plan.boundary, BoundaryPlan::Bridge(_)).then_some(*site)
        })
        .collect::<Vec<_>>();
    let call_bridge = (!call_sites.is_empty()).then(|| CallBridgeArtifactPlan {
        helper_symbol: generated_symbol_for(
            translation_unit_digest,
            "call_helper",
            ccc_sema::generic::FullFunctionId(u32::MAX),
            None,
        ),
        call_sites,
        frame_version: 2,
    });

    let mut bridge_entries = std::collections::BTreeMap::new();
    for (function, definition) in definitions {
        let BoundaryPlan::Bridge(boundary) = &definition.boundary else {
            continue;
        };
        if !matches!(
            boundary.kind,
            crate::BridgeKind::FixedEntry | crate::BridgeKind::VariadicEntry
        ) {
            return Err(AbiError::new(
                "CCC3515",
                "a bridged definition has a call-side bridge kind",
            ));
        }
        let source = module
            .functions
            .iter()
            .find(|candidate| candidate.id == *function)
            .ok_or_else(|| {
                AbiError::new(
                    "CCC3515",
                    format!(
                        "ABI definition {} is absent from the typed module",
                        function.0
                    ),
                )
            })?;
        let source_linkage = match source.linkage {
            ccc_sema::generic::Linkage::None => SourceLinkage::None,
            ccc_sema::generic::Linkage::Internal => SourceLinkage::Internal,
            ccc_sema::generic::Linkage::External => SourceLinkage::External,
        };
        let source_visibility = match source.visibility {
            ccc_sema::generic::SymbolVisibility::Default => SourceVisibility::Default,
            ccc_sema::generic::SymbolVisibility::Hidden => SourceVisibility::Hidden,
            ccc_sema::generic::SymbolVisibility::Protected => SourceVisibility::Protected,
            ccc_sema::generic::SymbolVisibility::Internal => SourceVisibility::Internal,
        };
        let source_binding = match source.binding {
            ccc_sema::generic::SymbolBinding::Strong => SourceBinding::Strong,
            ccc_sema::generic::SymbolBinding::Weak => SourceBinding::Weak,
        };
        bridge_entries.insert(
            *function,
            BridgeEntryArtifactPlan {
                function: *function,
                kind: boundary.kind,
                public_symbol: source.symbol_name.clone(),
                public_symbol_is_exact: source.symbol_name_is_exact,
                source_linkage,
                source_visibility,
                source_binding,
                body_symbol: generated_symbol_for(
                    translation_unit_digest,
                    match boundary.kind {
                        crate::BridgeKind::FixedEntry => "fixed_body",
                        crate::BridgeKind::VariadicEntry => "variadic_body",
                        _ => unreachable!(),
                    },
                    *function,
                    None,
                ),
                frame_version: 2,
                va_state_version: if abi_identity == ccc_target::AbiIdentity::SysvAmd64Lp64 {
                    1
                } else {
                    2
                },
            },
        );
    }

    let mut tls_accessors = std::collections::BTreeMap::new();
    for object in &module.globals {
        let is_tls = object.duration == ccc_sema::generic::StorageDuration::Thread
            || object.emission.tls.is_some();
        if !is_tls {
            continue;
        }
        if !required_symbols.objects.contains(&object.id) {
            continue;
        }
        if object.duration != ccc_sema::generic::StorageDuration::Thread {
            return Err(AbiError::new(
                "CCC3516",
                format!(
                    "TLS model on non-thread object `{}` is not a valid backend contract",
                    object.name
                ),
            )
            .with_span_if_none(object.span));
        }
        let source_linkage = match object.linkage {
            ccc_sema::generic::Linkage::None => SourceLinkage::None,
            ccc_sema::generic::Linkage::Internal => SourceLinkage::Internal,
            ccc_sema::generic::Linkage::External => SourceLinkage::External,
        };
        let source_visibility = match object.emission.visibility {
            ccc_sema::generic::SymbolVisibility::Default => SourceVisibility::Default,
            ccc_sema::generic::SymbolVisibility::Hidden => SourceVisibility::Hidden,
            ccc_sema::generic::SymbolVisibility::Protected => SourceVisibility::Protected,
            ccc_sema::generic::SymbolVisibility::Internal => SourceVisibility::Internal,
        };
        tls_accessors.insert(
            object.id,
            TlsAccessorArtifactPlan {
                object: object.id,
                object_symbol: object.emission.symbol_name.clone(),
                object_symbol_is_exact: object.emission.symbol_name_is_exact,
                helper_symbol: generated_symbol_for(
                    translation_unit_digest,
                    "tls_accessor",
                    ccc_sema::generic::FullFunctionId(object.id.0),
                    None,
                ),
                model: object
                    .emission
                    .tls
                    .unwrap_or(ccc_sema::generic::TlsModel::GeneralDynamic),
                source_linkage,
                source_visibility,
                source_defined: object.emission.definition
                    != ccc_sema::generic::ObjectDefinitionPolicy::Declaration,
            },
        );
    }

    let mut exact_localization_symbols = Vec::new();
    if let Some(call_bridge) = &call_bridge {
        exact_localization_symbols.push(call_bridge.helper_symbol.clone());
    }
    if let Some(support) = &f80_support {
        exact_localization_symbols.push(support.helper_symbol.clone());
    }
    if let Some(support) = &inline_asm_support {
        exact_localization_symbols.extend(support.cpuid_symbol.iter().cloned());
        exact_localization_symbols.extend(support.rdtsc_symbol.iter().cloned());
    }
    for entry in bridge_entries.values() {
        exact_localization_symbols.push(entry.body_symbol.clone());
        if matches!(
            entry.source_linkage,
            SourceLinkage::None | SourceLinkage::Internal
        ) {
            exact_localization_symbols.push(entry.public_symbol.clone());
        }
    }
    for accessor in tls_accessors.values() {
        exact_localization_symbols.push(accessor.helper_symbol.clone());
        if accessor.source_defined
            && matches!(
                accessor.source_linkage,
                SourceLinkage::None | SourceLinkage::Internal
            )
        {
            exact_localization_symbols.push(accessor.object_symbol.clone());
        }
    }
    exact_localization_symbols.sort();
    exact_localization_symbols.dedup();
    let generated_assembly_units = u32::try_from(
        usize::from(call_bridge.is_some())
            + usize::from(f80_support.is_some())
            + usize::from(inline_asm_support.is_some())
            + bridge_entries.len()
            + tls_accessors.len(),
    )
    .map_err(|_| AbiError::new("CCC3503", "generated assembly unit count overflow"))?;
    let needs_packaging = generated_assembly_units != 0;
    Ok(BridgeArtifactPlan {
        call_bridge,
        bridge_entries,
        tls_accessors,
        f80_support,
        inline_asm_support,
        packaging: PackagingPlan {
            generated_assembly_units,
            requires_assembler: needs_packaging,
            requires_relocatable_link: needs_packaging,
            requires_object_copier: needs_packaging
                && abi_identity != ccc_target::AbiIdentity::DarwinArm64,
            exact_localization_symbols,
        },
    })
}

fn lowered_signature(boundary: &BoundaryPlan) -> LoweredSignaturePlan {
    match boundary {
        BoundaryPlan::Native(plan) => LoweredSignaturePlan::Native {
            parameters: plan.clif_parameters.clone(),
            results: plan.clif_results.clone(),
        },
        BoundaryPlan::Bridge(_) => LoweredSignaturePlan::UniformFramePointer,
    }
}

impl ModuleAbiPlan {
    /// Recomputes all ABI-sensitive state and returns the capability token
    /// required by code generation.
    pub fn verify_against<'a>(
        &'a self,
        module: &gir::FullModule,
        config: &EffectiveCompilationConfig,
    ) -> Result<VerifiedModuleAbiPlan<'a>, AbiError> {
        let current = plan_module(module, config)?;
        if current != *self {
            return Err(AbiError::new(
                "CCC3515",
                "module ABI plan no longer matches the typed IR or effective ABI configuration",
            ));
        }
        Ok(VerifiedModuleAbiPlan { plan: self })
    }

    pub fn generated_symbol(
        &self,
        kind: &str,
        function: ccc_sema::generic::FullFunctionId,
        instruction: Option<gir::InstructionId>,
    ) -> String {
        generated_symbol_for(self.translation_unit_digest, kind, function, instruction)
    }
}

fn generated_symbol_for(
    translation_unit_digest: crate::TranslationUnitDigest,
    kind: &str,
    function: ccc_sema::generic::FullFunctionId,
    instruction: Option<gir::InstructionId>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ccc-generated-symbol-v1\0");
    hasher.update(translation_unit_digest.0);
    hasher.update((kind.len() as u64).to_le_bytes());
    hasher.update(kind.as_bytes());
    hasher.update(function.0.to_le_bytes());
    hasher.update(
        instruction
            .map_or(u32::MAX, |instruction| instruction.0)
            .to_le_bytes(),
    );
    format!("__ccc_{kind}_{}", hex(&hasher.finalize()))
}

pub fn dump_module_plan(verified: VerifiedModuleAbiPlan<'_>) -> String {
    let plan = verified.plan();
    let mut output = String::new();
    writeln!(output, "abi-plan schema={}", plan.config_key.schema).unwrap();
    writeln!(output, "target={}", plan.config_key.target_triple).unwrap();
    writeln!(
        output,
        "abi-identity={}",
        plan.config_key.abi_identity.name()
    )
    .unwrap();
    writeln!(output, "data-layout={}", plan.config_key.data_layout).unwrap();
    writeln!(
        output,
        "calling-convention={}",
        calling_convention_name(plan.config_key.calling_convention)
    )
    .unwrap();
    writeln!(
        output,
        "boundary-profile={}",
        plan.config_key.boundary_profile
    )
    .unwrap();
    writeln!(
        output,
        "classifier-revision={}",
        plan.config_key.classifier_revision
    )
    .unwrap();
    writeln!(
        output,
        "specification-revision={}",
        plan.config_key.specification_revision
    )
    .unwrap();
    writeln!(
        output,
        "specification-source-sha256={}",
        plan.config_key.specification_source_sha256
    )
    .unwrap();
    writeln!(
        output,
        "backend-profile={}",
        plan.config_key.backend_profile
    )
    .unwrap();
    writeln!(
        output,
        "normalized-target-arch={}",
        plan.config_key.normalized_target_arch
    )
    .unwrap();
    writeln!(
        output,
        "normalized-target-abi={}",
        plan.config_key.normalized_target_abi
    )
    .unwrap();
    writeln!(
        output,
        "normalized-target-cpu={}",
        plan.config_key.normalized_target_cpu
    )
    .unwrap();
    writeln!(
        output,
        "normalized-deployment-target={}",
        plan.config_key.normalized_deployment_target
    )
    .unwrap();
    writeln!(output, "ir-shape-sha256={}", plan.ir_shape_digest).unwrap();
    writeln!(
        output,
        "translation-unit-sha256={}",
        plan.translation_unit_digest
    )
    .unwrap();
    for (function, definition) in &plan.definitions {
        writeln!(
            output,
            "definition function={} source-signature={} source={}:{}..{}",
            function.0,
            definition.source_signature.index(),
            definition.source_location.file.index(),
            definition.source_location.start,
            definition.source_location.end
        )
        .unwrap();
        dump_lowered_signature(&mut output, &definition.lowered_signature, "  ");
        dump_boundary(&mut output, &definition.boundary, "  ");
    }
    for ((function, instruction), call) in &plan.calls {
        writeln!(
            output,
            "call function={} instruction={} target={} source-signature={} fixed-boundary={} actual-types={} source={}:{}..{}",
            function.0,
            instruction.0,
            match call.target {
                CallTarget::Direct(target) => format!("direct:{}", target.0),
                CallTarget::Indirect(value) => format!("indirect:v{}", value.0),
            },
            call.source_signature.index(),
            call.fixed_boundary,
            call.promoted_actual_types
                .iter()
                .map(|ty| ty.index().to_string())
                .collect::<Vec<_>>()
                .join(","),
            call.source_location.file.index(),
            call.source_location.start,
            call.source_location.end
        )
        .unwrap();
        dump_lowered_signature(&mut output, &call.lowered_signature, "  ");
        dump_boundary(&mut output, &call.boundary, "  ");
    }
    for ((function, instruction), va_arg) in &plan.va_args {
        writeln!(
            output,
            "va-arg function={} instruction={} type={} passing={} gp={} sse={} overflow-size={} overflow-align={}",
            function.0,
            instruction.0,
            va_arg.classified.ty.index(),
            passing_name(va_arg.classified.passing),
            va_arg.gp_slots,
            va_arg.sse_slots,
            va_arg.overflow_size,
            va_arg.overflow_align
        )
        .unwrap();
        dump_pieces(&mut output, &va_arg.classified, "  ", "va-arg-piece");
    }
    dump_artifacts(&mut output, &plan.artifacts);
    output
}

fn dump_lowered_signature(output: &mut String, signature: &LoweredSignaturePlan, indent: &str) {
    match signature {
        LoweredSignaturePlan::UniformFramePointer => {
            writeln!(output, "{indent}lowered-signature=uniform-frame-pointer-v1").unwrap();
        }
        LoweredSignaturePlan::Native {
            parameters,
            results,
        } => {
            writeln!(
                output,
                "{indent}lowered-signature=native parameter-count={} result-count={}",
                parameters.len(),
                results.len()
            )
            .unwrap();
            for carrier in parameters {
                writeln!(
                    output,
                    "{indent}lowered-parameter {}",
                    render_native_carrier(carrier)
                )
                .unwrap();
            }
            for carrier in results {
                writeln!(
                    output,
                    "{indent}lowered-result {}",
                    render_native_carrier(carrier)
                )
                .unwrap();
            }
        }
    }
}

fn dump_artifacts(output: &mut String, artifacts: &BridgeArtifactPlan) {
    if let Some(call_bridge) = &artifacts.call_bridge {
        writeln!(
            output,
            "call-bridge helper={} frame-version={} sites={}",
            call_bridge.helper_symbol,
            call_bridge.frame_version,
            call_bridge
                .call_sites
                .iter()
                .map(|(function, instruction)| format!("{}:{}", function.0, instruction.0))
                .collect::<Vec<_>>()
                .join(",")
        )
        .unwrap();
    } else {
        writeln!(output, "call-bridge none").unwrap();
    }
    for entry in artifacts.bridge_entries.values() {
        writeln!(
            output,
            "bridge-entry function={} kind={} public={} exact={} linkage={} visibility={} binding={} body={} frame-version={} va-state-version={}",
            entry.function.0,
            match entry.kind {
                crate::BridgeKind::FixedEntry => "fixed-entry",
                crate::BridgeKind::VariadicEntry => "variadic-entry",
                _ => "invalid-call-side-kind",
            },
            entry.public_symbol,
            entry.public_symbol_is_exact,
            source_linkage_name(entry.source_linkage),
            source_visibility_name(entry.source_visibility),
            match entry.source_binding {
                SourceBinding::Strong => "strong",
                SourceBinding::Weak => "weak",
            },
            entry.body_symbol,
            entry.frame_version,
            entry.va_state_version
        )
        .unwrap();
    }
    for accessor in artifacts.tls_accessors.values() {
        writeln!(
            output,
            "tls-accessor object={} symbol={} exact={} helper={} model={} linkage={} visibility={} defined={}",
            accessor.object.0,
            accessor.object_symbol,
            accessor.object_symbol_is_exact,
            accessor.helper_symbol,
            tls_model_name(accessor.model),
            source_linkage_name(accessor.source_linkage),
            source_visibility_name(accessor.source_visibility),
            accessor.source_defined,
        )
        .unwrap();
    }
    if let Some(support) = &artifacts.inline_asm_support {
        writeln!(
            output,
            "inline-asm-support cpuid={} rdtsc={}",
            support.cpuid_symbol.as_deref().unwrap_or("none"),
            support.rdtsc_symbol.as_deref().unwrap_or("none")
        )
        .unwrap();
    } else {
        writeln!(output, "inline-asm-support none").unwrap();
    }
    writeln!(
        output,
        "packaging assembly-units={} assembler={} relocatable-link={} object-copier={} exact-localize={}",
        artifacts.packaging.generated_assembly_units,
        artifacts.packaging.requires_assembler,
        artifacts.packaging.requires_relocatable_link,
        artifacts.packaging.requires_object_copier,
        artifacts.packaging.exact_localization_symbols.join(",")
    )
    .unwrap();
}

fn tls_model_name(model: ccc_sema::generic::TlsModel) -> &'static str {
    match model {
        ccc_sema::generic::TlsModel::GeneralDynamic => "global-dynamic",
        ccc_sema::generic::TlsModel::LocalDynamic => "local-dynamic",
        ccc_sema::generic::TlsModel::InitialExec => "initial-exec",
        ccc_sema::generic::TlsModel::LocalExec => "local-exec",
    }
}

fn source_linkage_name(linkage: SourceLinkage) -> &'static str {
    match linkage {
        SourceLinkage::None => "none",
        SourceLinkage::Internal => "internal",
        SourceLinkage::External => "external",
    }
}

fn source_visibility_name(visibility: SourceVisibility) -> &'static str {
    match visibility {
        SourceVisibility::Default => "default",
        SourceVisibility::Hidden => "hidden",
        SourceVisibility::Protected => "protected",
        SourceVisibility::Internal => "elf-internal",
    }
}

fn calling_convention_name(convention: ccc_target::CallingConvention) -> &'static str {
    match convention {
        ccc_target::CallingConvention::SystemV => "system-v",
        ccc_target::CallingConvention::WindowsFastcall => "windows-fastcall",
        ccc_target::CallingConvention::AppleAarch64 => "apple-aarch64",
        ccc_target::CallingConvention::WasmBasicCAbi => "wasm-basic-c-abi",
        _ => "unknown",
    }
}

fn dump_boundary(output: &mut String, boundary: &BoundaryPlan, indent: &str) {
    match boundary {
        BoundaryPlan::Native(native) => {
            writeln!(
                output,
                "{indent}transport=native placement-authority=cranelift"
            )
            .unwrap();
            writeln!(
                output,
                "{indent}clif-signature params={} results={}",
                native
                    .clif_parameters
                    .iter()
                    .map(render_native_carrier)
                    .collect::<Vec<_>>()
                    .join(","),
                native
                    .clif_results
                    .iter()
                    .map(render_native_carrier)
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap();
            for parameter in &native.parameters {
                let action = match parameter.classified.passing {
                    PassingMode::Scalar => "scalar".to_owned(),
                    PassingMode::Registers => "reconstruct-register-pieces".to_owned(),
                    PassingMode::Memory => {
                        let padded = parameter
                            .carrier_indices
                            .iter()
                            .find_map(|index| {
                                match native.clif_parameters[*index as usize].purpose {
                                    NativePurpose::StructArgument(size) => Some(size),
                                    NativePurpose::IndirectArgument => {
                                        u32::try_from(parameter.classified.size).ok()
                                    }
                                    _ => None,
                                }
                            })
                            .unwrap_or(0);
                        format!(
                            "copy-logical-bytes:{}-to-padded:{padded}",
                            parameter.classified.size
                        )
                    }
                    PassingMode::Void => "invalid-void".to_owned(),
                };
                writeln!(
                    output,
                    "{indent}parameter source={} type={} size={} align={} passing={} classes={} carriers={} action={action}",
                    parameter.source_index,
                    parameter.ty.index(),
                    parameter.classified.size,
                    parameter.classified.align,
                    passing_name(parameter.classified.passing),
                    render_classes(&parameter.classified.classes),
                    parameter
                        .carrier_indices
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
                .unwrap();
                dump_pieces(output, &parameter.classified, indent, "parameter-piece");
            }
            match &native.result {
                crate::NativeResultPlan::Void => {
                    writeln!(output, "{indent}result passing=void action=none").unwrap();
                }
                crate::NativeResultPlan::Scalar { ty, carrier_index } => {
                    writeln!(
                        output,
                        "{indent}result type={} passing=scalar carrier={} action=scalar",
                        ty.index(),
                        carrier_index
                    )
                    .unwrap();
                }
                crate::NativeResultPlan::RegisterAggregate {
                    classified,
                    carrier_indices,
                } => {
                    writeln!(
                        output,
                        "{indent}result type={} size={} align={} passing=registers classes={} carriers={} action=materialize-owned-result",
                        classified.ty.index(),
                        classified.size,
                        classified.align,
                        render_classes(&classified.classes),
                        carrier_indices
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                    .unwrap();
                    dump_pieces(output, classified, indent, "result-piece");
                }
                crate::NativeResultPlan::Indirect {
                    classified,
                    sret_parameter_index,
                } => {
                    writeln!(
                        output,
                        "{indent}result type={} size={} align={} passing=memory classes={} hidden-return=true sret-parameter={} action=fresh-nonaliasing-storage",
                        classified.ty.index(),
                        classified.size,
                        classified.align,
                        render_classes(&classified.classes),
                        sret_parameter_index
                    )
                    .unwrap();
                    dump_pieces(output, classified, indent, "result-piece");
                }
            }
        }
        BoundaryPlan::Bridge(bridge) => {
            writeln!(
                output,
                "{indent}transport=bridge abi={} kind={} stack-size={} overflow-arg-offset={} gp-used={} xmm-used={} al={} hidden-return={}",
                bridge.abi_identity.name(),
                match bridge.kind {
                    crate::BridgeKind::UnprototypedCall => "unprototyped-call",
                    crate::BridgeKind::VariadicCall => "variadic-call",
                    crate::BridgeKind::VariadicEntry => "variadic-entry",
                    crate::BridgeKind::FixedCall => "fixed-call",
                    crate::BridgeKind::FixedEntry => "fixed-entry",
                },
                bridge.stack_size,
                bridge.overflow_arg_offset,
                bridge.gp_used,
                bridge.xmm_used,
                bridge.variadic_sse_count,
                bridge.hidden_return
            )
            .unwrap();
            for (source, parameter) in bridge.parameters.iter().enumerate() {
                writeln!(
                    output,
                    "{indent}parameter source={source} type={} size={} align={} passing={} classes={}",
                    parameter.ty.index(),
                    parameter.size,
                    parameter.align,
                    passing_name(parameter.passing),
                    render_classes(&parameter.classes)
                )
                .unwrap();
            }
            for piece in &bridge.parameter_pieces {
                writeln!(
                    output,
                    "{indent}parameter-piece source={} index={} class={} offset={} valid={} extension={} indirect={} location={}",
                    piece
                        .source_index
                        .map_or_else(|| "result".to_owned(), |index| index.to_string()),
                    piece.piece.index,
                    class_name(piece.piece.class),
                    piece.piece.offset,
                    piece.piece.valid_bytes,
                    extension_name(piece.extension),
                    piece.indirect,
                    render_location(piece.location)
                )
                .unwrap();
            }
            writeln!(
                output,
                "{indent}result type={} size={} align={} passing={} classes={} hidden-return={}",
                bridge.result.ty.index(),
                bridge.result.size,
                bridge.result.align,
                passing_name(bridge.result.passing),
                render_classes(&bridge.result.classes),
                bridge.hidden_return
            )
            .unwrap();
            for piece in &bridge.result_pieces {
                writeln!(
                    output,
                    "{indent}result-piece index={} class={} offset={} valid={} extension={} indirect={} location={}",
                    piece.piece.index,
                    class_name(piece.piece.class),
                    piece.piece.offset,
                    piece.piece.valid_bytes,
                    extension_name(piece.extension),
                    piece.indirect,
                    render_location(piece.location)
                )
                .unwrap();
            }
        }
    }
}

fn dump_pieces(output: &mut String, classified: &crate::ClassifiedType, indent: &str, label: &str) {
    for piece in &classified.pieces {
        writeln!(
            output,
            "{indent}{label} index={} class={} offset={} valid={}",
            piece.index,
            class_name(piece.class),
            piece.offset,
            piece.valid_bytes
        )
        .unwrap();
    }
}

fn render_native_carrier(carrier: &crate::NativeCarrierPlan) -> String {
    let purpose = match carrier.purpose {
        NativePurpose::Normal => "normal".to_owned(),
        NativePurpose::StructArgument(size) => format!("sarg({size})"),
        NativePurpose::IndirectArgument => "indirect-argument".to_owned(),
        NativePurpose::StructReturn => "sret".to_owned(),
        NativePurpose::Padding => "padding".to_owned(),
    };
    format!(
        "abi={} source={} piece={} offset={} valid={} class={} carrier={} extension={} purpose={}",
        carrier.abi_param_index,
        carrier
            .source_index
            .map_or_else(|| "none".to_owned(), |index| index.to_string()),
        carrier
            .piece_index
            .map_or_else(|| "none".to_owned(), |index| index.to_string()),
        carrier.source_offset,
        carrier.valid_bytes,
        class_name(carrier.class),
        match carrier.carrier {
            AbiCarrier::I8 => "i8",
            AbiCarrier::I16 => "i16",
            AbiCarrier::I32 => "i32",
            AbiCarrier::I64 => "i64",
            AbiCarrier::I128 => "i128",
            AbiCarrier::F16 => "f16",
            AbiCarrier::F32 => "f32",
            AbiCarrier::F64 => "f64",
            AbiCarrier::V32 => "v32",
            AbiCarrier::V64 => "v64",
        },
        extension_name(carrier.extension),
        purpose
    )
}

fn extension_name(extension: crate::IntegerExtension) -> &'static str {
    match extension {
        crate::IntegerExtension::None => "none",
        crate::IntegerExtension::Signed => "signed",
        crate::IntegerExtension::Unsigned => "unsigned",
    }
}

fn render_classes(classes: &[AbiClass]) -> String {
    classes
        .iter()
        .map(|class| class_name(*class))
        .collect::<Vec<_>>()
        .join("+")
}

fn class_name(class: AbiClass) -> &'static str {
    match class {
        AbiClass::NoClass => "NO_CLASS",
        AbiClass::Integer => "INTEGER",
        AbiClass::Sse => "SSE",
        AbiClass::SseUp => "SSEUP",
        AbiClass::X87 => "X87",
        AbiClass::X87Up => "X87UP",
        AbiClass::ComplexX87 => "COMPLEX_X87",
        AbiClass::Memory => "MEMORY",
    }
}

fn passing_name(passing: PassingMode) -> &'static str {
    match passing {
        PassingMode::Void => "void",
        PassingMode::Scalar => "scalar",
        PassingMode::Registers => "registers",
        PassingMode::Memory => "memory",
    }
}

fn render_location(location: BridgeLocation) -> String {
    match location {
        BridgeLocation::Register(register) => format!(
            "{}:{}",
            match register.bank {
                crate::RegisterBank::Integer => "integer",
                crate::RegisterBank::Float => "float",
                crate::RegisterBank::X87 => "x87",
            },
            register.index
        ),
        BridgeLocation::Stack { offset } => format!("stack:+{offset}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_bridge_selection_treats_darwin_long_double_as_binary64() {
        let mut types = ccc_types::TypeStore::default();
        let signature = types.function_type(ccc_types::FunctionType::prototype(
            ccc_types::QualifiedType::unqualified(ccc_types::TypeId::LONG_DOUBLE),
            vec![ccc_types::QualifiedType::unqualified(
                ccc_types::TypeId::LONG_DOUBLE,
            )],
        ));
        assert!(function_signature_requires_bridge(
            &types,
            signature,
            &EffectiveCompilationConfig::default(),
        ));
        assert!(!function_signature_requires_bridge(
            &types,
            signature,
            &EffectiveCompilationConfig::aarch64_apple_darwin(),
        ));
    }

    #[test]
    fn generated_names_use_the_full_translation_unit_digest() {
        let module = gir::FullModule {
            types: ccc_types::TypeStore::default(),
            globals: Vec::new(),
            strings: Vec::new(),
            functions: Vec::new(),
        };
        let plan = plan_module(&module, &EffectiveCompilationConfig::default()).unwrap();
        let symbol = plan.generated_symbol("bridge", ccc_sema::generic::FullFunctionId(4), None);
        assert!(symbol.starts_with("__ccc_bridge_"));
        assert_eq!(symbol.len(), "__ccc_bridge_".len() + 64);
        let verified = plan
            .verify_against(&module, &EffectiveCompilationConfig::default())
            .unwrap();
        assert!(dump_module_plan(verified).contains("abi-plan schema=ccc-abi-config-v3"));
    }

    #[test]
    fn verification_rejects_ir_mutation() {
        let mut module = gir::FullModule {
            types: ccc_types::TypeStore::default(),
            globals: Vec::new(),
            strings: Vec::new(),
            functions: Vec::new(),
        };
        let plan = plan_module(&module, &EffectiveCompilationConfig::default()).unwrap();
        module.strings.push(gir::FullString {
            id: ccc_sema::generic::StringId(0),
            encoding: gir::StringEncoding::Ordinary,
            code_units: vec![0],
            ty: ccc_types::QualifiedType::unqualified(ccc_types::TypeId::CHAR),
        });
        assert_eq!(
            plan.verify_against(&module, &EffectiveCompilationConfig::default())
                .unwrap_err()
                .code,
            "CCC3515"
        );
    }
}
