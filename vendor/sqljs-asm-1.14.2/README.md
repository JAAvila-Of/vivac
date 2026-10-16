# Fixed SQLite frontend runtime

Source: [sql.js v1.14.2](https://github.com/sql-js/sql.js/releases/tag/v1.14.2),
upstream commit `9c4e167`. `sql-asm.js` and `LICENSE` are the original release
bytes. The asm.js build does not require WebAssembly. The complete runtime is
retained with each SQLite application revision; it is not fetched at runtime.

| File | Bytes | SHA-256 |
|---|---:|---|
| `sql-asm.js` | 1,355,169 | `82344159a161e30e1c012587a59567adc02f537cd5f376270eff88c564eed0fd` |
| `LICENSE` | 2,199 | `60a3f6e4d7b29b4321359e683b36cf198d24f58e24582070f56e6fa89d5ee2be` |
| `vivac-sqlite.js.deflate` | 453,701 | `87b4ee801f9d4574e047094065d19ba2b8772a2e7f3f136a4e72179cb5beeb37` |

The uncompressed `vivac-sqlite.js` is exactly 1,357,427 bytes with SHA-256
`a5709877905821fcae8aeb6789c53302eb0ffc50473f81dc2bfbdd93a16f9b38`.
Its recipe concatenates these bytes without line-ending normalization:

1. UTF-8 `/*! sql.js 1.14.2. Upstream runtime follows unchanged.` and LF.
2. The original `LICENSE` bytes.
3. LF, `*/`, LF.
4. The original `sql-asm.js` bytes.

The packed file uses raw DEFLATE, level 9. It was produced with Node 24.21.0
and zlib `1.3.2.1-motley-8002e91`. To reproduce it from this directory:

```javascript
const fs = require('node:fs');
const zlib = require('node:zlib');
const bytes = Buffer.concat([
  Buffer.from('/*! sql.js 1.14.2. Upstream runtime follows unchanged.\n'),
  fs.readFileSync('LICENSE'),
  Buffer.from('\n*/\n'),
  fs.readFileSync('sql-asm.js'),
]);
const packed = zlib.deflateRawSync(bytes, { level: 9 });
// Compare both lengths and SHA-256 digests above before replacing any asset.
```

Only the packed fixed asset is embedded. Materialization streams it with exact
compressed and uncompressed digest and length checks. Caller-supplied files,
paths, origins or hashes cannot add trusted catalog entries. Ordinary artifact
guards still apply to application files and database contents; this dependency
is reviewed as product code. Retained runtime reads require an exact match to
the compiled historical catalog. Adding or changing an identity requires a
product dependency review, including license, provenance, execution boundary,
performance and supported distribution builds.
