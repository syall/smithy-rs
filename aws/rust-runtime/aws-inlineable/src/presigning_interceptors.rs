/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

#![allow(dead_code)]

use crate::presigning::PresigningConfig;
use crate::serialization_settings::HeaderSerializationSettings;
use aws_runtime::auth::sigv4::{HttpSignatureType, SigV4OperationSigningConfig};
use aws_runtime::invocation_id::InvocationIdInterceptor;
use aws_runtime::request_info::RequestInfoInterceptor;
use aws_runtime::user_agent::UserAgentInterceptor;
use aws_sigv4::http_request::SignableBody;
use aws_smithy_async::time::{SharedTimeSource, StaticTimeSource};
use aws_smithy_runtime::client::retries::strategy::NeverRetryStrategy;
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::{
    BeforeSerializationInterceptorContextMut, BeforeTransmitInterceptorContextMut,
};
use aws_smithy_runtime_api::client::interceptors::{
    disable_interceptor, Interceptor, SharedInterceptor,
};
use aws_smithy_runtime_api::client::retries::SharedRetryStrategy;
use aws_smithy_runtime_api::client::runtime_components::{
    RuntimeComponents, RuntimeComponentsBuilder,
};
use aws_smithy_runtime_api::client::runtime_plugin::RuntimePlugin;
use aws_smithy_types::config_bag::{ConfigBag, FrozenLayer, Layer};
use std::borrow::Cow;

/// Interceptor that tells the SigV4 signer to add the signature to query params,
/// and sets the request expiration time from the presigning config.
#[derive(Debug)]
pub(crate) struct SigV4PresigningInterceptor {
    config: PresigningConfig,
    payload_override: SignableBody<'static>,
}

impl SigV4PresigningInterceptor {
    pub(crate) fn new(config: PresigningConfig, payload_override: SignableBody<'static>) -> Self {
        Self {
            config,
            payload_override,
        }
    }
}

impl Interceptor for SigV4PresigningInterceptor {
    fn modify_before_serialization(
        &self,
        _context: &mut BeforeSerializationInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        cfg.interceptor_state()
            .store_put::<HeaderSerializationSettings>(
                HeaderSerializationSettings::new()
                    .omit_default_content_length()
                    .omit_default_content_type(),
            );
        Ok(())
    }

    fn modify_before_signing(
        &self,
        _context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if let Some(mut config) = cfg.load::<SigV4OperationSigningConfig>().cloned() {
            config.signing_options.expires_in = Some(self.config.expires());
            config.signing_options.signature_type = HttpSignatureType::HttpRequestQueryParams;
            config.signing_options.payload_override = Some(self.payload_override.clone());
            cfg.interceptor_state()
                .store_put::<SigV4OperationSigningConfig>(config);
            Ok(())
        } else {
            Err(
                "SigV4 presigning requires the SigV4OperationSigningConfig to be in the config bag. \
                This is a bug. Please file an issue.".into(),
            )
        }
    }
}

/// Runtime plugin that registers the SigV4PresigningInterceptor.
#[derive(Debug)]
pub(crate) struct SigV4PresigningRuntimePlugin {
    runtime_components: RuntimeComponentsBuilder,
}

impl SigV4PresigningRuntimePlugin {
    pub(crate) fn new(config: PresigningConfig, payload_override: SignableBody<'static>) -> Self {
        let time_source = SharedTimeSource::new(StaticTimeSource::new(config.start_time()));
        Self {
            runtime_components: RuntimeComponentsBuilder::new("SigV4PresigningRuntimePlugin")
                .with_interceptor(SharedInterceptor::new(SigV4PresigningInterceptor::new(
                    config,
                    payload_override,
                )))
                .with_retry_strategy(Some(SharedRetryStrategy::new(NeverRetryStrategy::new())))
                .with_time_source(Some(time_source)),
        }
    }
}

impl RuntimePlugin for SigV4PresigningRuntimePlugin {
    fn config(&self) -> Option<FrozenLayer> {
        let mut layer = Layer::new("Presigning");
        layer.store_put(disable_interceptor::<InvocationIdInterceptor>("presigning"));
        layer.store_put(disable_interceptor::<RequestInfoInterceptor>("presigning"));
        layer.store_put(disable_interceptor::<UserAgentInterceptor>("presigning"));
        Some(layer.freeze())
    }

    fn runtime_components(&self) -> Cow<'_, RuntimeComponentsBuilder> {
        Cow::Borrowed(&self.runtime_components)
    }
}