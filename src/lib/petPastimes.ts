/**
 * The corner pet's pastimes as Settings → 桌宠 shows them: a thumbnail and a name for each.
 * The ones switched off are kept in localStorage (shared by the main window and the pet windows),
 * and PetWindow leaves them out of its shuffled rounds.
 */
export const PET_PASTIMES_OFF_KEY = 'bento-pet-pastimes-off';

export const PET_LOOK = { src: '/pet/c-sit.webp', name: 'Bento', description: '毛绒小 Bento，戴着面罩坐在小木凳上' };

export const PET_PASTIMES: { id: string; label: string; thumb: string; note?: string }[] = [
  { id: 'drink', label: '喝咖啡', thumb: '/pet/c-mug-sip.webp' },
  { id: 'code', label: '敲代码', thumb: '/pet/c-tap-a.webp' },
  { id: 'read', label: '看书', thumb: '/pet/c-book-flat.webp' },
  { id: 'water', label: '浇花', thumb: '/pet/c-water-pour.webp' },
  { id: 'sword', label: '舞剑', thumb: '/pet/c-sword-raise.webp' },
  { id: 'music', label: '听音乐', thumb: '/pet/c-music.webp' },
  { id: 'stretch', label: '伸懒腰', thumb: '/pet/c-stretch.webp' },
  { id: 'cube', label: '玩魔方', thumb: '/pet/c-cube-b.webp' },
  { id: 'doze', label: '打盹', thumb: '/pet/c-doze.webp' },
  { id: 'bubbles', label: '吹泡泡', thumb: '/pet/c-bubble-a.webp' },
  { id: 'plane', label: '纸飞机', thumb: '/pet/c-plane-b.webp' },
  { id: 'snack', label: '吃零食', thumb: '/pet/c-snack-b.webp' },
  { id: 'scarf', label: '围巾', thumb: '/pet/c-scarf-2.webp', note: '仅冬季' },
];

export function readPastimesOff(): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(PET_PASTIMES_OFF_KEY) ?? '[]');
    return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
  } catch {
    return [];
  }
}

export function writePastimesOff(off: string[]) {
  try { localStorage.setItem(PET_PASTIMES_OFF_KEY, JSON.stringify(off)); } catch { /* storage full or blocked */ }
}
