import pinyin from 'tiny-pinyin';

const han = /\p{Script=Han}/u;
type Spellings = { full: string; initials: string };
const byEntry = new WeakMap<object, Map<string, Spellings>>();
const byLabel = new Map<string, Spellings>();

function spellings(text: string) {
  const compact = text.toLowerCase().replace(/[\s_-]/gu, '');
  if (!han.test(compact) || !pinyin.isSupported()) return { full: compact, initials: compact };
  const tokens = pinyin.parse(compact);
  return {
    full: tokens.map(token => token.target.toLowerCase()).join(''),
    initials: tokens.map(token => (token.type === 2 ? token.target[0] : token.source).toLowerCase()).join(''),
  };
}

export function pinyinMatcher(query: string) {
  // Leave a pure Chinese query on the existing exact/fuzzy path.
  if (!/[a-z]/i.test(query)) return () => false;
  const target = spellings(query);
  return (text: string, entry?: object) => {
    if (!han.test(text)) return false;
    let cache = byLabel;
    if (entry) {
      cache = byEntry.get(entry) ?? new Map();
      byEntry.set(entry, cache);
    }
    let key = cache.get(text);
    if (!key) {
      key = spellings(text);
      if (!entry && cache.size >= 2_048) cache.delete(cache.keys().next().value!);
      cache.set(text, key);
    }
    return key.full.includes(target.full) || key.initials.includes(target.initials);
  };
}
