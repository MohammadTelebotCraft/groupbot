from pathlib import Path
import re
root = Path(__file__).resolve().parents[2]
path = root / 'src/state.rs'
source = path.read_text(encoding='utf-8')
match = re.search(r'        const FOLD: &str = "(.*?)";', source, flags=re.S)
assert match and '\\' not in match[1]
(root / 'src/state/counter_fold.sql').write_text(match[1]+'\n', encoding='utf-8')
source = source[:match.start()] + '        const FOLD: &str = include_str!("state/counter_fold.sql");' + source[match.end():]
path.write_text(source, encoding='utf-8')
