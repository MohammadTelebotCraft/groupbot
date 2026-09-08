// Exercise the actual Mini App render functions without a network or Telegram account.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.resolve(__dirname, '..');
const source = fs.readFileSync(path.join(root, 'src/miniapp/assets/app.js'), 'utf8');
const rows = JSON.parse(fs.readFileSync(path.join(root, 'src/handlers/premium/registry.json'), 'utf8'));
const fallbacks = Object.fromEntries(rows.map(row => [row.key, row.fallback]));

function functionSource(name) {
  const start = source.indexOf('  function ' + name + '(');
  assert(start >= 0, name);
  // These standalone render functions end at the two-space indentation level.
  const end = source.indexOf('\n  }', start) + '\n  }'.length;
  return source.slice(start, end);
}

const context = vm.createContext({
  window: { MODERATION_ICONS: fallbacks },
  S: { dash: { chat: { title: 'گروه Test 🔕' } } },
  renderPageTop: () => '', loadingList: () => '',
});
vm.runInContext(
  'const ICONS = {};\n' + functionSource('esc') + '\n' + functionSource('icon') + '\n' +
  functionSource('renderRightsPage') + '\n' + functionSource('renderPickField'), context,
);
for (const [name, key] of [['ban', 'MODERATION_HAMMER'], ['mute', 'MUTED'], ['clock', 'TIMER'],
                         ['fileText', 'DOCUMENT_ACTIVITY'], ['MIC_MUTED', 'MIC_MUTED']]) {
  const html = context.icon(name, 18);
  assert(html.includes(`data-icon="${key}"`));
  assert(html.includes(fallbacks[key]));
  assert(html.includes('aria-hidden="true"'));
  assert(!/\d{18}/.test(html));
}

const rights = [
  { key: 'photos', label: 'ارسال عکس', open: false, icon: 'IMAGE_DISABLED' },
  { key: 'voices', label: 'ارسال ویس', open: false, icon: 'MIC_MUTED' },
  { key: 'plain', label: 'Send پیام', open: true, icon: 'UNLOCKED' },
];
const page = context.renderRightsPage({ data: { rights } });
for (const right of rights) {
  assert(page.includes(`data-right="${right.key}"`));
  assert(page.includes(`data-icon="${right.icon}"`));
  assert(page.includes(right.label));
}
const picks = context.renderPickField({ label: 'برخورد', chosen: 'mute', options: [
  { id: 'fl_mute', value: 'mute', label: 'سکوت', icon: 'MUTED' },
  { id: 'fl_ban', value: 'ban', label: 'بن', icon: 'MODERATION_HAMMER', danger: true },
] });
assert(picks.includes('data-apply="fl_mute"'));
assert(picks.includes('data-icon="MUTED"'));
assert(picks.includes('data-apply="fl_ban"'));
assert(picks.includes('data-icon="MODERATION_HAMMER"'));
assert(picks.includes('chip on'));
assert(picks.includes('chip danger'));
const css = fs.readFileSync(path.join(root, 'src/miniapp/assets/app.css'), 'utf8');
assert(/\.semantic-icon\s*\{[^}]*direction:\s*ltr;[^}]*unicode-bidi:\s*isolate;/s.test(css));
console.log('Premium UI: icon rendering, mixed RTL labels, permission states and unchanged action attributes passed.');
