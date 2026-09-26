<script lang="ts" module>
  // 16×16 line icons drawn with a 1.5px stroke (no icon font, no external requests).
  const PATHS = {
    pin: "M9.6 2.4 13.6 6.4 11.3 7.5 9.4 9.4 9.9 12.1 8.9 13.1 2.9 7.1 3.9 6.1 6.6 6.6 8.5 4.7ZM5.9 10.1 2.5 13.5",
    collapse: "M2.5 9.5H6.5V13.5M13.5 6.5H9.5V2.5M6.5 9.5 2.5 13.5M9.5 6.5 13.5 2.5",
    expand: "M9.5 2.5H13.5V6.5M6.5 13.5H2.5V9.5M13.5 2.5 9.5 6.5M2.5 13.5 6.5 9.5",
    tune: "M2.5 4.5H8M12 4.5H13.5M2.5 11.5H4M8 11.5H13.5M10 2.5V6.5M6 9.5V13.5",
    back: "M13 8H3.5M7.5 4 3.5 8 7.5 12",
    lock: "M4 7.5H12V13.5H4ZM5.75 7.5V5.25A2.25 2.25 0 0 1 10.25 5.25V7.5",
    terminal: "M2 3.5H14V12.5H2ZM4.5 6.25 6.5 8 4.5 9.75M8.25 10H11",
    desktop: "M1.75 3H14.25V11H1.75ZM5.5 13.5H10.5M8 11V13.5",
    cowork: "M8 1.75 9.3 6.7 14.25 8 9.3 9.3 8 14.25 6.7 9.3 1.75 8 6.7 6.7Z",
    warning: "M8 2.25 14.25 13.25H1.75ZM8 6.5V9.25M8 11.25V11.3",
    info: "M8 1.75A6.25 6.25 0 1 1 7.99 1.75ZM8 7.25V11M8 5V5.05",
    check: "M3.25 8.5 6.5 11.5 12.75 4.75",
    close: "M4 4 12 12M12 4 4 12",
    plus: "M8 3V13M3 8H13",
    minus: "M3 8H13",
    folder: "M1.75 3.75H6.25L7.75 5.25H14.25V12.75H1.75Z",
    power: "M8 1.75V7.5M4.6 3.9A5.25 5.25 0 1 0 11.4 3.9",
    shield: "M8 1.75 13.25 3.5V7.5C13.25 10.6 11 12.95 8 14.25 5 12.95 2.75 10.6 2.75 7.5V3.5Z",
    ghost: "M3.5 13.5V7A4.5 4.5 0 0 1 12.5 7V13.5L11 12.25 9.5 13.5 8 12.25 6.5 13.5 5 12.25ZM6.25 7.25V7.3M9.75 7.25V7.3",
    link: "M6.5 9.5 9.5 6.5M7.25 4.25 8.5 3A2.47 2.47 0 0 1 13 7.5L11.75 8.75M8.75 11.75 7.5 13A2.47 2.47 0 0 1 3 8.5L4.25 7.25",
    update: "M8 1.75A6.25 6.25 0 1 1 7.99 1.75ZM8 11V5.25M5.5 7.75 8 5.25 10.5 7.75",
    external: "M9.5 2.5H13.5V6.5M13.5 2.5 8 8M11.5 9.5V13.5H2.5V4.5H6.5",
  } as const;

  export type IconName = keyof typeof PATHS;
</script>

<script lang="ts">
  interface Props {
    name: IconName;
    size?: number;
    /** Fill the shape (e.g. a pressed pin). */
    filled?: boolean;
  }

  let { name, size = 16, filled = false }: Props = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 16 16"
  aria-hidden="true"
  focusable="false"
>
  <path
    d={PATHS[name]}
    fill={filled ? "currentColor" : "none"}
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
  />
</svg>

<style>
  .icon {
    display: block;
    flex: none;
  }
</style>
