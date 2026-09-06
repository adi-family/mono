import { Building2, Eye, GraduationCap } from '@lucide/astro';

// One Lucide icon per flag kind (design/DESIGN.md §9: Lucide only, stroke 1.5). None of these
// three nouns are in §9's noun→icon table, but follow its spirit: Preview reads as "look before
// it's finished", Expert area as a body of specialist knowledge, Enterprise interest as an
// organization rather than an individual.
export const FLAG_ICONS = {
	Preview: Eye,
	'Expert area': GraduationCap,
	'Enterprise interest': Building2,
} as const;

export type FlagKind = keyof typeof FLAG_ICONS;
