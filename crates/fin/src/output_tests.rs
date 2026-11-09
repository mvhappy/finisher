//! `output.rs`'s unit tests (kept apart so the module stays under the size limit).
use super::*;

#[test]
fn exit_codes_follow_the_contract() {
    assert_eq!(exit_code("USAGE"), 2);
    assert_eq!(exit_code("APPROVAL_PENDING"), 8);
    assert_eq!(exit_code("APPROVAL_DENIED"), 7);
    assert_eq!(exit_code("ROLE_UNKNOWN"), 3);
    assert_eq!(exit_code("PROVIDER_AUTH"), 5);
    assert_eq!(exit_code("TOOL_TIMEOUT"), 5);
    assert_eq!(exit_code("AGENT_ARGS"), 2);
    assert_eq!(exit_code("TICKET_LIMIT"), 5);
    assert_eq!(exit_code("CREDENTIAL_LIMIT"), 4);
    assert_eq!(exit_code("APPROVAL_LIMIT"), 4);
    assert_eq!(exit_code("APPROVAL_REQUIRED"), 7);
    assert_eq!(exit_code("CREDENTIAL_REVOKED"), 7);
    assert_eq!(exit_code("CREDENTIAL_EXPIRED"), 7);
    assert_eq!(exit_code("BRANCH_IN_USE"), 4);
    assert_eq!(exit_code("BRANCH_NOT_ALLOWED"), 7);
    assert_eq!(exit_code("TX_TOO_LARGE"), 2);
    assert_eq!(exit_code("CONFLICT"), 4);
    assert_eq!(exit_code("NEIGHBOURS_GONE"), 4);
    assert_eq!(exit_code("SEGMENT_COLLISION"), 4);
    assert_eq!(exit_code("RECORD_TOO_LARGE"), 3);
    assert_eq!(exit_code("BASE_REV_AHEAD"), 4);
    assert_eq!(exit_code("PROJECT_READ_ONLY"), 4);
    assert_eq!(exit_code("LEASE_TAKEOVER_REFUSED"), 4);
    assert_eq!(exit_code("LEASE_LOST"), 4);
    assert_eq!(exit_code("HISTORY_REWOUND"), 4);
    assert_eq!(exit_code("TX_SET_ASIDE"), 4);
    assert_eq!(exit_code("UNSUPPORTED_HASH"), 3);
    for code in [
        "UNSUPPORTED_CONTAINER",
        "UNSUPPORTED_TRANSFORM",
        "MEDIA_CORRUPT",
        "MEDIA_LIMIT",
        "EXPORT_DESTINATION",
    ] {
        assert_eq!(exit_code(code), 3, "{code}");
    }
    assert_eq!(exit_code("VERIFY_FAILED"), 3);
    assert_eq!(exit_code("DB_IN_USE"), 5);
    assert_eq!(exit_code("CONTROL_DB_BUSY"), 5);
    assert_eq!(exit_code("CONSENT_UI_ONLY"), 2);
    assert_eq!(exit_code("JOURNAL_UNAVAILABLE"), 5);
    assert_eq!(exit_code("SETTINGS_INVALID"), 5);
    assert_eq!(exit_code("SETTINGS_WRITE_FAILED"), 5);
    assert_eq!(exit_code("EVENT_KIND_UNKNOWN"), 3);
    assert_eq!(exit_code("EVENT_INVALID"), 3);
    assert_eq!(exit_code("EVENT_TOO_LARGE"), 3);
    assert_eq!(exit_code("FONT_FETCH_FAILED"), 5);
    for code in [
        "GPU_UNAVAILABLE",
        "GPU_DEVICE_LOST",
        "GPU_OUT_OF_MEMORY",
        "ENCODER_HUNG",
    ] {
        assert_eq!(exit_code(code), 5, "{code}");
    }
    assert_eq!(exit_code("FONTS_DAMAGED"), 5);
    assert_eq!(exit_code("SHM_REFUSED"), 5);
    // Registries (8A Task 9a, spec §13): 3 for what is not there or malformed, 4 for a
    // clash, 5 for unreachable or a suspect clock, 7 when a person must look.
    for (code, exit) in [
        ("REGISTRY_NOT_FOUND", 3),
        ("REGISTRY_INVALID", 3),
        ("PLUGIN_NOT_FOUND", 3),
        ("REGISTRY_EXISTS", 4),
        ("REGISTRY_UNREACHABLE", 5),
        ("CLOCK_SUSPECT", 5),
        ("REGISTRY_UNTRUSTED", 7),
        ("TUF_SIGNATURE", 7),
        ("TUF_THRESHOLD", 7),
        ("TUF_ROLLBACK", 7),
        ("TUF_EXPIRED", 7),
        ("TUF_DELEGATION", 7),
        ("TUF_TARGET_MISMATCH", 7),
        // Plugins (8A, spec §13).
        ("MANIFEST_TOO_LARGE", 2),
        ("REGISTRY_AMBIGUOUS", 2),
        ("PLUGIN_MANIFEST_INVALID", 3),
        ("PLUGIN_CONTRIBUTION_UNSUPPORTED", 3),
        ("PACKAGE_UNSAFE", 3),
        ("PACKAGE_TOO_LARGE", 3),
        ("PLUGIN_HOST_API", 3),
        ("PLUGIN_PLATFORM", 3),
        ("PLUGIN_NODE_VERSION", 3),
        ("PLAN_EXPIRED", 3),
        ("PLAN_MISMATCH", 3),
        ("PLUGIN_NOT_INSTALLED", 3),
        ("PLUGIN_ALREADY_INSTALLED", 3),
        ("PLUGIN_DOWNGRADE_REFUSED", 3),
        ("PLUGIN_BUNDLED", 3),
        ("REGISTRY_STALE", 5),
        ("PLUGIN_DISABLED", 5),
        ("PLUGIN_RUNTIME_UNAVAILABLE", 5),
        ("TOO_MANY_PLANS", 5),
        ("TOO_MANY_SUGGESTIONS", 5),
        ("PLUGIN_REVOKED", 7),
        ("PLUGIN_BUNDLED_MISMATCH", 7),
        ("COPYLEFT_NOT_ACCEPTED", 7),
        ("THIRD_PARTY_CODE_NOT_ACCEPTED", 7),
    ] {
        assert_eq!(exit_code(code), exit, "{code}");
    }
    assert_eq!(exit_code("TRACK_LOCKED"), 3);
    assert_eq!(exit_code("OVERWRITE_SPLITS_ITEM"), 3);
    assert_eq!(exit_code("NOT_ADJACENT"), 3);
    for code in [
        "ALREADY_LINKED",
        "SLIDE_BLOCKED",
        "MOVE_INTO_ITSELF",
        "LINK_LEGS_OVERLAP",
        "TRANSITION_NO_HANDLES",
        "TRANSITION_EXISTS",
        "TRANSITION_INVALID",
        "TRACK_NOT_EMPTY",
        "TIMELINE_NOT_EMPTY",
        "TIMELINE_NESTED",
        "PARAM_UNKNOWN",
        "PARAM_TYPE",
        "SLOT_UNKNOWN",
        "PARAM_ANIMATED",
        "PARAM_NOT_ANIMATABLE",
        "KEYFRAME_EXISTS",
        "BIN_NOT_EMPTY",
        "BIN_CYCLE",
        "NESTED_CYCLE",
        "NESTED_TOO_LARGE",
    ] {
        assert_eq!(exit_code(code), 3, "{code}");
    }
    for code in [
        "SERVER_UNAVAILABLE",
        "SERVER_RUNNING",
        "SERVER_START_FAILED",
        "SERVER_TIMEOUT",
        "ENGINE_CLOSED",
        "ENGINE_BUSY",
        "SERVER_SHUTTING_DOWN",
        "CANCELLED",
    ] {
        assert_eq!(exit_code(code), 5, "{code}");
    }
    for code in [
        "INTERNAL",
        "SOMETHING_NEW",
        "PROTOCOL_ERROR",
        "PROTOCOL_MISMATCH",
        "DB_ERROR",
        "IO_ERROR",
    ] {
        assert_eq!(exit_code(code), 6, "{code}");
    }
    for code in [
        "PROJECT_NOT_FOUND",
        "PROJECT_EXISTS",
        "PROJECT_INVALID",
        "BRANCH_NOT_FOUND",
        "PROJECT_DB_MISSING",
        "PROJECT_DB_MISMATCH",
        "SERVER_DATA_UNSAFE",
        "SERVER_DATA_NOT_LOCAL",
        "PROJECT_DIR_UNWRITABLE",
        "INVALID_NAME",
        "INVALID_PATH",
        "IDEM_KEY_REUSED",
        "DB_TOO_NEW",
        "NOT_A_PROJECT_DB",
        "NEWER_FORMAT",
        "INVALID_FORMAT",
        "REPLAY_FAILED",
        "CLIP_OVERLAP",
        "CLIP_EMPTY",
        "CLIP_NEGATIVE",
        "INVALID_ID",
        "BINDINGS_MISMATCH",
        "PROJECT_COPY",
        "PROJECT_ORIGINAL_UNREACHABLE",
        "NAME_TAKEN",
        "BRANCH_ARCHIVED",
        "NOTHING_TO_UNDO",
        "NOTHING_TO_REDO",
        "FONT_INVALID",
        "FONT_NOT_IN_CATALOG",
        "FONT_CATALOG_INVALID",
    ] {
        assert_eq!(exit_code(code), 3, "{code}");
    }
    for code in VALIDATION_CODES {
        assert_eq!(exit_code(code), 3, "{code}");
    }
}

/// 5E Decision 26: the review codes and the protected-branch refusal.
#[test]
fn review_codes_follow_decision_26() {
    for code in ["REVIEW_NOT_FOUND", "REVIEW_UNDECIDED"] {
        assert_eq!(exit_code(code), 3, "{code}");
    }
    for code in [
        "REVIEW_STALE",
        "REVIEW_HAS_CONFLICTS",
        "REVIEW_CLOSED",
        "REVIEW_LIMIT",
        "AUTO_MERGE_UNAVAILABLE",
    ] {
        assert_eq!(exit_code(code), 4, "{code}");
    }
    assert_eq!(exit_code("POLICY_BUSY"), 5);
    assert_eq!(exit_code("BRANCH_PROTECTED"), 7);
}

#[test]
fn job_codes_follow_decision_41() {
    for code in [
        "INVALID_REQUEST",
        "JOB_KIND_UNKNOWN",
        "JOB_PARAMS_INVALID",
        "JOB_NOT_FOUND",
        "JOBHOST_STATE",
    ] {
        assert_eq!(exit_code(code), 3, "{code}");
    }
    for code in JOB_RESULT_CODES {
        assert_eq!(exit_code(code), 4, "{code}");
    }
    for code in [
        "JOB_QUEUE_FULL",
        "WORKER_UNAVAILABLE",
        "WORKER_PROTOCOL_MISMATCH",
        "WAIT_TIMEOUT",
        "RESOURCE_STUCK",
        "WORKER_OOM",
    ] {
        assert_eq!(exit_code(code), 5, "{code}");
    }
    // Only in `reply` errors to a worker: never an exit of their own.
    for code in ["UNKNOWN_MESSAGE", "CALL_LIMIT"] {
        assert_eq!(exit_code(code), 6, "{code}");
    }
}
