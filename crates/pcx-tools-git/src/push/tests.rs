// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_module_api::sandbox::Root;

use super::*;

fn config(sock: &str) -> GitPush {
    GitPush {
        remote: String::new(),
        branches: Vec::new(),
        ssh_auth_sock: sock.to_string(),
        known_hosts: String::new(),
    }
}

fn policy(network: bool, roots: &[&str]) -> Policy {
    Policy {
        roots: roots
            .iter()
            .map(|r| Root {
                path: PathBuf::from(r),
                write: true,
            })
            .collect(),
        network,
        spawn: true,
        deny: Vec::new(),
        local_only: false,
    }
}

#[test]
fn no_push_configured_is_none() {
    assert!(Push::of(None).unwrap().is_none());
}

#[test]
fn by_default_origin_and_the_tasks_branches() {
    let push = Push::of(Some(config("/run/agent/sock"))).unwrap().unwrap();
    assert_eq!(push.remote, "origin");
    assert!(push.allowed("task/12ab-fix-it").is_ok());
    for refused in ["main", "feat/cloud-and-plugins", "task/a/b", "tasks/1"] {
        assert!(push.allowed(refused).is_err(), "{refused}");
    }
}

#[test]
fn the_configured_branches_are_the_ones() {
    let mut c = config("/run/agent/sock");
    c.branches = vec!["agent/*".to_string(), "release-?".to_string()];
    let push = Push::of(Some(c)).unwrap().unwrap();
    assert!(push.allowed("agent/x").is_ok());
    assert!(push.allowed("release-1").is_ok());
    assert!(push.allowed("task/1").is_err());
}

#[test]
fn a_push_without_a_socket_an_option_or_a_relative_path_is_refused() {
    assert!(Push::of(Some(config(""))).is_err());
    assert!(Push::of(Some(config("agent.sock"))).is_err());
    assert!(Push::of(Some(config("/run/my agent/sock"))).is_err());
    let mut remote = config("/run/agent/sock");
    remote.remote = "--upload-pack=evil".to_string();
    assert!(Push::of(Some(remote)).is_err());
    let mut hosts = config("/run/agent/sock");
    hosts.known_hosts = "known_hosts".to_string();
    assert!(Push::of(Some(hosts)).is_err());
}

#[test]
fn a_push_needs_the_network_and_the_socket_under_a_root() {
    let push = Push::of(Some(config("/run/agent/sock"))).unwrap().unwrap();
    assert!(
        push.reachable(&policy(true, &["/repo", "/run/agent"]))
            .is_ok()
    );
    let offline = push.reachable(&policy(false, &["/repo", "/run/agent"]));
    assert_eq!(
        offline.unwrap_err().kind,
        ModuleErrorKind::FailedPrecondition
    );
    let unreachable = push.reachable(&policy(true, &["/repo"]));
    assert_eq!(
        unreachable.unwrap_err().kind,
        ModuleErrorKind::FailedPrecondition
    );
}

#[test]
fn ssh_never_asks_and_checks_the_remotes_key() {
    let mut c = config("/run/agent/sock");
    let env = Push::of(Some(c.clone())).unwrap().unwrap().env();
    assert!(env.contains(&("SSH_AUTH_SOCK".to_string(), "/run/agent/sock".to_string())));
    let ssh = &env.iter().find(|(k, _)| k == "GIT_SSH_COMMAND").unwrap().1;
    assert!(
        ssh.contains("BatchMode=yes") && ssh.contains("accept-new"),
        "{ssh}"
    );
    c.known_hosts = "/run/agent/known_hosts".to_string();
    let env = Push::of(Some(c)).unwrap().unwrap().env();
    let ssh = &env.iter().find(|(k, _)| k == "GIT_SSH_COMMAND").unwrap().1;
    assert!(
        ssh.contains("StrictHostKeyChecking=yes") && ssh.contains("/run/agent/known_hosts"),
        "{ssh}"
    );
}
