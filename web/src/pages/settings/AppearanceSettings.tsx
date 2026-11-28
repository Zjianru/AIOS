import { useAppStore, type ThemeMode } from '@/app/store';

const THEME_OPTIONS: { value: ThemeMode; label: string }[] = [
  { value: 'system', label: '跟随系统' },
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
];

export function AppearanceSettings() {
  const themeMode = useAppStore((state) => state.themeMode);
  const reducedMotion = useAppStore((state) => state.reducedMotion);
  const setThemeMode = useAppStore((state) => state.setThemeMode);
  const setTheme = useAppStore((state) => state.setTheme);
  const setReducedMotion = useAppStore((state) => state.setReducedMotion);
  const chooseTheme = (mode: ThemeMode) => {
    setThemeMode(mode);
    if (mode !== 'system') setTheme(mode);
  };
  const motionValue = reducedMotion === null ? 'system' : reducedMotion ? 'on' : 'off';

  return (
    <section className="mt-6" data-settings="appearance">
      <h2 className="text-3 font-medium text-ink">外观</h2>
      <p className="mt-1 text-2 leading-5 text-ink-2">主题和减少动效保存在这台机器上，重开还在。</p>

      <div className="mt-4 rounded-card border border-line p-3">
        <div className="text-2 text-ink">主题</div>
        <div className="mt-2 flex flex-wrap gap-2">
          {THEME_OPTIONS.map((option) => (
            <button
              key={option.value}
              type="button"
              data-appearance-theme={option.value}
              aria-pressed={themeMode === option.value}
              onClick={() => chooseTheme(option.value)}
              className={
                'rounded-btn border px-3 py-1.5 text-2 ' +
                (themeMode === option.value
                  ? 'border-ink/30 bg-active text-ink'
                  : 'border-line text-ink-2 hover:bg-hover hover:text-ink')
              }
            >
              {option.label}
            </button>
          ))}
        </div>
      </div>

      <div className="mt-3 rounded-card border border-line p-3">
        <label className="text-2 text-ink" htmlFor="appearance-motion">减少动效</label>
        <select
          id="appearance-motion"
          data-appearance-motion
          value={motionValue}
          onChange={(event) => {
            const value = event.target.value;
            setReducedMotion(value === 'system' ? null : value === 'on');
          }}
          className="ml-3 rounded-btn border border-line bg-pane px-2 py-1 text-2 text-ink outline-none"
        >
          <option value="system">跟随系统</option>
          <option value="on">开</option>
          <option value="off">关</option>
        </select>
        <p className="mt-2 text-1 leading-4 text-ink-3">跟随系统时读 macOS 的「减弱动态效果」。</p>
      </div>
    </section>
  );
}
