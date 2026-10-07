/** @type {import('tailwindcss').Config} */
const token = (name) => `rgb(var(--${name}) / <alpha-value>)`;

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        "surface-0": token("surface-0"),
        "surface-1": token("surface-1"),
        "surface-2": token("surface-2"),
        "surface-3": token("surface-3"),
        line: token("line"),
        fg: token("fg"),
        "fg-muted": token("fg-muted"),
        accent: token("accent"),
      },
    },
  },
  plugins: [],
};
