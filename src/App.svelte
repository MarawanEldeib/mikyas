<script lang="ts">
  // M0 spike UI: static mock values used only to judge window effects and RAM.
  const windows = [
    { label: "5h", pct: 29, reset: "3h 12m" },
    { label: "7d", pct: 59, reset: "2d 4h" },
  ];
  const R = 18;
  const C = 2 * Math.PI * R;
  const color = (p: number) => (p >= 70 ? "var(--red)" : p >= 40 ? "var(--orange)" : "var(--green)");
</script>

<main class="card">
  <header>
    <span class="chip">Opus 5.5 · 1M</span>
    <span class="muted">ctx 34%</span>
  </header>
  <section class="rings">
    {#each windows as w}
      <div class="ring">
        <svg viewBox="0 0 44 44" width="56" height="56">
          <circle cx="22" cy="22" r={R} fill="none" stroke="var(--hairline)" stroke-width="5" />
          <circle
            cx="22" cy="22" r={R} fill="none" stroke={color(w.pct)} stroke-width="5" stroke-linecap="round"
            stroke-dasharray={C} stroke-dashoffset={C * (1 - w.pct / 100)} transform="rotate(-90 22 22)"
          />
          <text x="22" y="26" text-anchor="middle" font-size="11" fill="currentColor">{w.pct}%</text>
        </svg>
        <div>
          <div class="label">{w.label}</div>
          <div class="muted">resets in {w.reset}</div>
        </div>
      </div>
    {/each}
  </section>
  <footer class="muted">spike · effect preview</footer>
</main>

<style>
  .card {
    box-sizing: border-box;
    height: 100%;
    padding: 12px 14px;
    background: var(--surface);
    border: 1px solid var(--hairline);
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  header { display: flex; justify-content: space-between; align-items: center; font-size: 12px; }
  .chip { padding: 2px 8px; border-radius: 999px; background: var(--hairline); font-weight: 600; }
  .rings { display: flex; flex-direction: column; gap: 8px; }
  .ring { display: flex; align-items: center; gap: 10px; }
  .label { font-weight: 600; font-size: 13px; }
  .muted { color: var(--muted); font-size: 12px; }
  footer { margin-top: auto; font-size: 11px; }
</style>
