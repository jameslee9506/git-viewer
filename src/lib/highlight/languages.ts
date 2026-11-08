// File path → shiki language id. A static table: shiki must not be imported on the main thread, and
// bundledLanguagesInfo carries no file extensions anyway. Every value is a canonical bundledLanguages id.

/** Lowercase extension (without the dot) → language id. */
const EXT = new Map<string, string>(
  Object.entries({
    ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'tsx',
    js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'jsx',
    json: 'json', geojson: 'json', topojson: 'json', webmanifest: 'json', har: 'json', avsc: 'json',
    jsonc: 'jsonc', 'code-workspace': 'jsonc', json5: 'json5', jsonl: 'jsonl', ndjson: 'jsonl', hjson: 'hjson',
    yaml: 'yaml', yml: 'yaml', toml: 'toml',
    ini: 'ini', cfg: 'ini', conf: 'ini', properties: 'ini', env: 'dotenv',
    xml: 'xml', xsd: 'xml', plist: 'xml', svg: 'xml', rss: 'xml', atom: 'xml', wsdl: 'xml', xlf: 'xml',
    xliff: 'xml', csproj: 'xml', fsproj: 'xml', vbproj: 'xml', vcxproj: 'xml', props: 'xml', targets: 'xml',
    resx: 'xml', xaml: 'xml', storyboard: 'xml', xib: 'xml', entitlements: 'xml', nuspec: 'xml', iml: 'xml',
    xsl: 'xsl', xslt: 'xsl', csv: 'csv', tsv: 'tsv',
    md: 'markdown', markdown: 'markdown', mkd: 'markdown', mdown: 'markdown', mkdn: 'markdown', mdx: 'mdx',
    rst: 'rst', adoc: 'asciidoc', asciidoc: 'asciidoc', org: 'org', typ: 'typst',
    tex: 'latex', ltx: 'latex', sty: 'latex', cls: 'latex', bib: 'bibtex',
    html: 'html', htm: 'html', xhtml: 'html', vue: 'vue', svelte: 'svelte', astro: 'astro',
    hbs: 'handlebars', handlebars: 'handlebars', mustache: 'handlebars', twig: 'twig', liquid: 'liquid',
    j2: 'jinja', jinja: 'jinja', jinja2: 'jinja', njk: 'jinja', erb: 'erb', haml: 'haml', pug: 'pug',
    jade: 'pug', cshtml: 'razor', razor: 'razor', marko: 'marko', wiki: 'wikitext', po: 'po', pot: 'po',
    css: 'css', scss: 'scss', sass: 'sass', less: 'less', styl: 'stylus', stylus: 'stylus',
    pcss: 'postcss', postcss: 'postcss',
    rs: 'rust', go: 'go', c: 'c', h: 'c',
    cpp: 'cpp', cc: 'cpp', cxx: 'cpp', 'c++': 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp', 'h++': 'cpp',
    ipp: 'cpp', inl: 'cpp', tpp: 'cpp', ino: 'cpp', cu: 'cpp', cuh: 'cpp',
    m: 'objective-c', mm: 'objective-cpp', swift: 'swift',
    cs: 'csharp', csx: 'csharp', fs: 'fsharp', fsi: 'fsharp', fsx: 'fsharp', vb: 'vb',
    java: 'java', kt: 'kotlin', kts: 'kotlin', groovy: 'groovy', gvy: 'groovy', gradle: 'groovy',
    scala: 'scala', sc: 'scala', sbt: 'scala', clj: 'clojure', cljs: 'clojure', cljc: 'clojure', edn: 'clojure',
    dart: 'dart', zig: 'zig', nim: 'nim', nims: 'nim', d: 'd', odin: 'odin', vala: 'vala', cr: 'crystal',
    hs: 'haskell', lhs: 'haskell', elm: 'elm', ml: 'ocaml', mli: 'ocaml', purs: 'purescript',
    ex: 'elixir', exs: 'elixir', erl: 'erlang', hrl: 'erlang', gleam: 'gleam',
    lua: 'lua', luau: 'luau', py: 'python', pyi: 'python', pyw: 'python', pyx: 'python',
    bzl: 'python', bazel: 'python', star: 'python',
    rb: 'ruby', rake: 'ruby', gemspec: 'ruby', podspec: 'ruby', ru: 'ruby', rbi: 'ruby', rbs: 'rbs',
    php: 'php', phtml: 'php', pl: 'perl', pm: 'perl', raku: 'raku', rakumod: 'raku', p6: 'raku',
    r: 'r', jl: 'julia', hx: 'haxe', hxml: 'hxml', coffee: 'coffee', cson: 'coffee',
    sh: 'shellscript', bash: 'shellscript', zsh: 'shellscript', ksh: 'shellscript', command: 'shellscript',
    fish: 'fish', nu: 'nushell', ps1: 'powershell', psm1: 'powershell', psd1: 'powershell',
    bat: 'bat', cmd: 'bat', awk: 'awk', tcl: 'tcl', applescript: 'applescript',
    asm: 'asm', s: 'asm', nasm: 'asm', ll: 'llvm', wat: 'wasm', wast: 'wasm',
    wgsl: 'wgsl', glsl: 'glsl', vert: 'glsl', frag: 'glsl', geom: 'glsl', comp: 'glsl', tesc: 'glsl',
    tese: 'glsl', hlsl: 'hlsl', hlsli: 'hlsl', fxh: 'hlsl', shader: 'shaderlab',
    sol: 'solidity', move: 'move', cairo: 'cairo', vy: 'vyper',
    sql: 'sql', psql: 'sql', pgsql: 'sql', cypher: 'cypher', cql: 'cypher',
    graphql: 'graphql', gql: 'graphql', graphqls: 'graphql', proto: 'proto', prisma: 'prisma',
    tf: 'terraform', tfvars: 'terraform', hcl: 'hcl', nix: 'nix', cmake: 'cmake',
    mk: 'make', mak: 'make', make: 'make', dockerfile: 'docker', just: 'just', vim: 'viml',
    diff: 'diff', patch: 'diff', http: 'http', rest: 'http', log: 'log',
    tsp: 'typespec', bicep: 'bicep', cue: 'cue', kdl: 'kdl', ron: 'ron', pkl: 'pkl', smithy: 'smithy',
    templ: 'templ', qml: 'qml', gd: 'gdscript', gdshader: 'gdshader', tscn: 'gdresource', tres: 'gdresource',
    mermaid: 'mermaid', mmd: 'mermaid', lean: 'lean', sas: 'sas',
    f: 'fortran-fixed-form', for: 'fortran-fixed-form', f77: 'fortran-fixed-form',
    f90: 'fortran-free-form', f95: 'fortran-free-form', f03: 'fortran-free-form', f08: 'fortran-free-form',
    pas: 'pascal', dpr: 'pascal', pp: 'puppet', scm: 'scheme', ss: 'scheme', rkt: 'racket',
    lisp: 'common-lisp', lsp: 'common-lisp', el: 'emacs-lisp', fnl: 'fennel', hy: 'hy',
    cob: 'cobol', cbl: 'cobol', ada: 'ada', adb: 'ada', ads: 'ada', vhd: 'vhdl', vhdl: 'vhdl',
    sv: 'system-verilog', svh: 'system-verilog', reg: 'reg', desktop: 'desktop', kql: 'kusto',
    service: 'systemd', socket: 'systemd', timer: 'systemd', mount: 'systemd', ahk: 'ahk',
  }),
);

/** Two-part extensions checked before the last one. */
const EXT2 = new Map<string, string>(
  Object.entries({
    'blade.php': 'blade', 'html.erb': 'erb', 'html.twig': 'twig', 'html.j2': 'jinja', 'html.jinja': 'jinja',
  }),
);

/** Exact lowercase file names; '' = never highlight. */
const NAME = new Map<string, string>(
  Object.entries({
    '.gitignore': '', '.gitattributes': '', '.editorconfig': '',
    dockerfile: 'docker', containerfile: 'docker',
    makefile: 'make', gnumakefile: 'make', bsdmakefile: 'make', 'cmakelists.txt': 'cmake',
    justfile: 'just', '.justfile': 'just',
    gemfile: 'ruby', rakefile: 'ruby', podfile: 'ruby', vagrantfile: 'ruby', brewfile: 'ruby', fastfile: 'ruby',
    appfile: 'ruby', matchfile: 'ruby', guardfile: 'ruby', capfile: 'ruby', berksfile: 'ruby', dangerfile: 'ruby',
    jenkinsfile: 'groovy', 'build.bazel': 'python', workspace: 'python', 'workspace.bazel': 'python',
    'module.bazel': 'python',
    'cargo.lock': 'toml', 'uv.lock': 'toml', 'poetry.lock': 'toml', pipfile: 'toml', 'pipfile.lock': 'json',
    '.babelrc': 'json5', 'settings.json': 'jsonc', 'launch.json': 'jsonc', 'tasks.json': 'jsonc',
    'extensions.json': 'jsonc', 'keybindings.json': 'jsonc', 'devcontainer.json': 'jsonc',
    '.devcontainer.json': 'jsonc',
    '.gitmodules': 'ini', '.gitconfig': 'ini', '.npmrc': 'ini', '.yarnrc': 'ini', '.flake8': 'ini',
    '.pylintrc': 'ini', '.coveragerc': 'ini', '.clang-format': 'yaml', '.clang-tidy': 'yaml',
    '.envrc': 'shellscript', '.bashrc': 'shellscript', '.bash_profile': 'shellscript',
    '.bash_login': 'shellscript', '.bash_logout': 'shellscript', '.bash_aliases': 'shellscript',
    '.profile': 'shellscript', '.zshrc': 'shellscript', '.zshenv': 'shellscript', '.zprofile': 'shellscript',
    '.zlogin': 'shellscript', '.zlogout': 'shellscript', '.kshrc': 'shellscript', pkgbuild: 'shellscript',
    apkbuild: 'shellscript', gradlew: 'shellscript', mvnw: 'shellscript',
    '.vimrc': 'viml', _vimrc: 'viml', '.gvimrc': 'viml', '.exrc': 'viml',
    'nginx.conf': 'nginx', '.htaccess': 'apache', 'httpd.conf': 'apache', 'apache2.conf': 'apache',
    ssh_config: 'ssh-config', sshd_config: 'ssh-config',
    codeowners: 'codeowners', commit_editmsg: 'git-commit', merge_msg: 'git-commit', tag_editmsg: 'git-commit',
    'git-rebase-todo': 'git-rebase',
  }),
);

/** Suffixes that only wrap the real file type (`config.h.in`, `app.yml.dist`). */
const WRAPPER = new Set(['in', 'dist', 'example', 'sample', 'template', 'orig']);

const TSCONFIG = /^[tj]sconfig(\..+)?\.json$/;

/** Shiki language id for a file path, or null when unsupported. */
export function languageForPath(path: string): string | null {
  const base = path.slice(path.lastIndexOf('/') + 1).toLowerCase();
  if (base === 'config' && path.toLowerCase().endsWith('.ssh/config')) return 'ssh-config';
  return forName(base, true);
}

function forName(base: string, unwrap: boolean): string | null {
  const named = NAME.get(base);
  if (named !== undefined) return named || null;
  if (base === '.env' || base.startsWith('.env.')) return 'dotenv';
  if (base.startsWith('dockerfile.') || base.startsWith('containerfile.')) return 'docker';
  if (TSCONFIG.test(base)) return 'jsonc';
  const dot = base.lastIndexOf('.');
  if (dot <= 0) return null;
  const ext = base.slice(dot + 1);
  const dot2 = base.lastIndexOf('.', dot - 1);
  const lang = (dot2 > 0 && EXT2.get(base.slice(dot2 + 1))) || EXT.get(ext);
  if (lang) return lang;
  return unwrap && WRAPPER.has(ext) ? forName(base.slice(0, dot), false) : null;
}
