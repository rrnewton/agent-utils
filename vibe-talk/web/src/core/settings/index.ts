// The device's settings, and apart from them its credentials. Its public interface.
// `#624 voice-page-modularity`.

export { createCredentials } from "./credentials.ts";
export type { Credentials } from "./credentials.ts";
export { IDENTITY_KEY, SYNCED_SETTING_KEYS } from "./keys.ts";
export { createSettingsSync, mergedIdentities } from "./sync.ts";
export type { AgreedSetting, SettingsSync, SettingsSyncDependencies } from "./sync.ts";
export { DEVICE_SETTINGS, createDeviceSettings } from "./device.ts";
export type { DeviceSetting, DeviceSettings } from "./device.ts";
