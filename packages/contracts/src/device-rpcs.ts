// 5B's RPCs (plan 5B Decision 1): pairing, devices, the environment and `settings.write`,
// spread into `FinRpcs` (`rpc.ts`) so that file stays small. Their rules are in `scopes.ts`.
import { Schema } from "effect";
import { Rpc } from "effect/rpc";
import {
  DeviceListPayload,
  DeviceListSuccess,
  DeviceRenamePayload,
  DeviceRenameSuccess,
  DeviceRevokePayload,
  DeviceRevokeSuccess,
  DevicesWatchItem,
  EnvConfirmPayload,
  EnvironmentSchema,
  EnvNamePayload,
  EnvResetSuccess,
  EnvRotateIdentitySuccess,
  PairApproveSuccess,
  PairOfferPayload,
  PairOfferSuccess,
  PairPendingIdPayload,
  PairPendingItem,
  PairRejectSuccess,
  SettingsWritePayload,
  SettingsWriteSuccess,
} from "./devices.ts";
import { FinError } from "./errors.ts";

export const DEVICE_RPCS = [
  Rpc.make("auth.pair.offer", { payload: PairOfferPayload, success: PairOfferSuccess, error: FinError }),
  Rpc.make("auth.pair.approve", { payload: PairPendingIdPayload, success: PairApproveSuccess, error: FinError }),
  Rpc.make("auth.pair.reject", { payload: PairPendingIdPayload, success: PairRejectSuccess, error: FinError }),
  Rpc.make("auth.pair.pending", { payload: Schema.Struct({}), success: PairPendingItem, error: FinError, stream: true }),
  Rpc.make("auth.device.list", { payload: DeviceListPayload, success: DeviceListSuccess, error: FinError }),
  Rpc.make("auth.device.rename", { payload: DeviceRenamePayload, success: DeviceRenameSuccess, error: FinError }),
  Rpc.make("auth.device.revoke", { payload: DeviceRevokePayload, success: DeviceRevokeSuccess, error: FinError }),
  Rpc.make("auth.devices.watch", { payload: Schema.Struct({}), success: DevicesWatchItem, error: FinError, stream: true }),
  Rpc.make("env.describe", { payload: Schema.Struct({}), success: EnvironmentSchema, error: FinError }),
  Rpc.make("env.name", { payload: EnvNamePayload, success: EnvironmentSchema, error: FinError }),
  Rpc.make("env.adopt", { payload: EnvConfirmPayload, success: EnvironmentSchema, error: FinError }),
  Rpc.make("env.reset", { payload: EnvConfirmPayload, success: EnvResetSuccess, error: FinError }),
  Rpc.make("env.rotateIdentity", { payload: EnvConfirmPayload, success: EnvRotateIdentitySuccess, error: FinError }),
  Rpc.make("settings.write", { payload: SettingsWritePayload, success: SettingsWriteSuccess, error: FinError }),
] as const;
