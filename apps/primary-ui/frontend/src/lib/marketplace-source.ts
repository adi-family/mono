import { createMachineChoice, normalizeMachine } from "./machine-choice";

/**
 * Which machine's marketplace is being browsed. A catalog, its sources and its installs are
 * one machine's, so unlike the chat list this is a single choice: a paired node's petname, or
 * `undefined` for this machine. Remembered across visits like the chat sources are.
 */
const choice = createMachineChoice("adi-marketplace-source");

export const normalizeMarketplaceSource = normalizeMachine;
export const readMarketplaceSource = choice.read;
export const setMarketplaceSource = choice.set;
export const useMarketplaceSource = choice.use;
