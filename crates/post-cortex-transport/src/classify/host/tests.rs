// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn host_calls_are_read_and_other_services_left_alone() {
    assert!(host("/pcx.v1.PostCortex/CreateSession", &[]).is_none());
    assert_eq!(
        host("/pcx.module.v1.Host/ListTools", &[]).unwrap(),
        Ok(HostCall::ListTools)
    );
    assert_eq!(
        host("/pcx.module.v1.Host/ListModules", &[]).unwrap(),
        Ok(HostCall::ListModules)
    );
    assert_eq!(
        host("/pcx.module.v1.Host/ListViews", &[]),
        Some(Ok(HostCall::ListViews))
    );
    assert_eq!(
        host("/pcx.module.v1.Host/GetView", &[]),
        Some(Ok(HostCall::GetView))
    );
    assert_eq!(
        host("/pcx.module.v1.Host/ModuleLog", &[]),
        Some(Ok(HostCall::ModuleLog))
    );
    assert_eq!(
        host("/pcx.module.v1.Host/ListModels", &[]),
        Some(Ok(HostCall::ListModels))
    );
    assert_eq!(
        host("/pcx.module.v1.Host/GetModule", &[]),
        Some(Ok(HostCall::GetModule))
    );
    let configure = wire::ConfigureModuleRequest {
        id: "greet".into(),
        config: None,
        clear: Vec::new(),
    };
    assert_eq!(
        host(
            "/pcx.module.v1.Host/ConfigureModule",
            &configure.encode_to_vec()
        )
        .unwrap(),
        Ok(HostCall::ConfigureModule { id: "greet".into() })
    );
    let grant = wire::GrantModuleRequest {
        id: "tools-git".into(),
        ..wire::GrantModuleRequest::default()
    };
    assert_eq!(
        host("/pcx.module.v1.Host/GrantModule", &grant.encode_to_vec()).unwrap(),
        Ok(HostCall::GrantModule {
            id: "tools-git".into()
        })
    );
    assert!(matches!(
        host("/pcx.module.v1.Host/Stop", &[]).unwrap(),
        Err(ClassifyError::UnknownRpc(_))
    ));
}
