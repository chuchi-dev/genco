// Monaco with syntax highlighting only: the editor worker and the basic
// languages, without the language services (their workers are megabytes)
import * as monaco from 'monaco-editor/esm/vs/editor/editor.api';
import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker';
import 'monaco-editor/esm/vs/basic-languages/css/css.contribution';
import 'monaco-editor/esm/vs/basic-languages/dockerfile/dockerfile.contribution';
import 'monaco-editor/esm/vs/basic-languages/html/html.contribution';
import 'monaco-editor/esm/vs/basic-languages/ini/ini.contribution';
import 'monaco-editor/esm/vs/basic-languages/javascript/javascript.contribution';
import 'monaco-editor/esm/vs/basic-languages/markdown/markdown.contribution';
import 'monaco-editor/esm/vs/basic-languages/php/php.contribution';
import 'monaco-editor/esm/vs/basic-languages/python/python.contribution';
import 'monaco-editor/esm/vs/basic-languages/rust/rust.contribution';
import 'monaco-editor/esm/vs/basic-languages/scss/scss.contribution';
import 'monaco-editor/esm/vs/basic-languages/shell/shell.contribution';
import 'monaco-editor/esm/vs/basic-languages/sql/sql.contribution';
import 'monaco-editor/esm/vs/basic-languages/twig/twig.contribution';
import 'monaco-editor/esm/vs/basic-languages/typescript/typescript.contribution';
import 'monaco-editor/esm/vs/basic-languages/xml/xml.contribution';
import 'monaco-editor/esm/vs/basic-languages/yaml/yaml.contribution';

self.MonacoEnvironment = {
	getWorker: () => new EditorWorker(),
};

export { monaco };

const LANGUAGES: Record<string, string> = {
	css: 'css',
	scss: 'scss',
	html: 'html',
	htm: 'html',
	twig: 'twig',
	js: 'javascript',
	mjs: 'javascript',
	cjs: 'javascript',
	json: 'javascript',
	ts: 'typescript',
	md: 'markdown',
	php: 'php',
	py: 'python',
	rs: 'rust',
	sh: 'shell',
	sql: 'sql',
	xml: 'xml',
	svg: 'xml',
	yml: 'yaml',
	yaml: 'yaml',
	ini: 'ini',
	toml: 'ini',
	conf: 'ini',
};

/** the monaco language of a file by its name */
export function languageFor(name: string): string {
	const lower = name.toLowerCase();
	if (lower === 'dockerfile') return 'dockerfile';
	if (lower.startsWith('.env') || lower.endsWith('.env')) return 'ini';
	const ext = lower.includes('.') ? lower.split('.').pop()! : '';
	return LANGUAGES[ext] ?? 'plaintext';
}
