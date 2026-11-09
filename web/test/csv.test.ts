import { describe, expect, it } from 'vitest';
import { csvToRows, parseCsv, rowsToCsv } from '../src/model/csv';
import { sizeLayout } from '../src/model/reel';
import { localize, setLang, t } from '../src/i18n';

const cols = [
  { id: 'label', label: 'Label', type: 'text' as const },
  { id: 'value', label: 'Value', type: 'number' as const },
];

describe('csv', () => {
  it('parses quotes, separators and headers', () => {
    expect(parseCsv('a,"b, c",d\n"x ""y""",2,3\n')).toEqual([['a', 'b, c', 'd'], ['x "y"', '2', '3']]);
    expect(parseCsv('a;b\n1;2')).toEqual([['a', 'b'], ['1', '2']]);
    expect(csvToRows('Label,Value\nNorth,5\nSouth,"9,5"', cols)).toEqual([{ label: 'North', value: 5 }, { label: 'South', value: 9.5 }]);
    expect(csvToRows('A\t1\nB\t2', cols)).toEqual([{ label: 'A', value: 1 }, { label: 'B', value: 2 }]);
  });
  it('round-trips rows', () => {
    const rows = [{ label: 'a, b', value: 1 }, { label: 'c', value: 2 }];
    expect(csvToRows(rowsToCsv(rows, cols), cols)).toEqual(rows);
  });
});

describe('sizes and i18n', () => {
  it('lays a size out at the script short side', () => {
    const info = { title: '', width: 1080, height: 1080, fps: 30, formats: [] };
    expect(sizeLayout(info, '1080x1920')).toEqual([1080, 1920]);
    expect(sizeLayout(info, '2480x3508')).toEqual([1080, 1528]);
    expect(sizeLayout(info, '1600x900')).toEqual([1920, 1080]);
  });
  it('translates UI strings and manifests', () => {
    setLang('de');
    expect(t('rendering', { p: 40 })).toBe('Wird erstellt… 40 %');
    const m = localize({ id: 'x', title: 'Quote', description: 'd', tags: [], formats: [], duration_s: 1, params: [{ id: 'q', type: 'text', label: 'Quote' }], i18n: { de: { title: 'Zitat', params: { q: 'Zitat' } } } });
    expect([m.title, m.params[0].label, m.description]).toEqual(['Zitat', 'Zitat', 'd']);
    setLang('en');
    expect(t('render')).toBe('Render video');
  });
});
