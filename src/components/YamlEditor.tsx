import CodeMirror, { EditorView } from "@uiw/react-codemirror";
import { yaml } from "@codemirror/lang-yaml";
import { useThemeStore } from "../stores/theme";

// Blend the editor chrome into the app's tokens; syntax colors come from the
// built-in light/dark themes.
const chrome = EditorView.theme({
  "&": { backgroundColor: "var(--bg-page)", fontSize: "12.5px", height: "100%" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.55" },
  ".cm-gutters": { backgroundColor: "var(--bg-surface)", borderRight: "1px solid var(--border-light)", color: "var(--text-muted)" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--bg-muted)" },
  "&.cm-focused": { outline: "none" },
});

const extensions = [yaml(), chrome, EditorView.lineWrapping];

export default function YamlEditor({
  value,
  onChange,
  readOnly = false,
}: {
  value: string;
  onChange?: (v: string) => void;
  readOnly?: boolean;
}) {
  const resolved = useThemeStore((s) => s.resolved);
  return (
    <CodeMirror
      value={value}
      onChange={onChange}
      readOnly={readOnly}
      theme={resolved}
      extensions={extensions}
      height="100%"
      className="h-full"
      basicSetup={{ foldGutter: true, highlightActiveLine: true, autocompletion: false }}
    />
  );
}
