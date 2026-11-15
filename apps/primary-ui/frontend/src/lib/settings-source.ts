import { createMachineChoice } from "./machine-choice";

/**
 * Whose settings are open. Triggers, like every setting that lives in a machine's store, belong
 * to one machine, so this is a single choice: a paired node's petname, or `undefined` for this
 * machine. Kept apart from the marketplace's choice — browsing another machine's catalog should
 * not silently change which machine a setting is saved on.
 */
const choice = createMachineChoice("adi-settings-source");

export const readSettingsSource = choice.read;
export const setSettingsSource = choice.set;
export const useSettingsSource = choice.use;
