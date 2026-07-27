/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        obsidian: {
          base: '#0B0F17',
          surface: '#111827',
          card: '#1F2937',
          border: 'rgba(255, 255, 255, 0.08)',
        },
        rose: {
          accent: '#F43F5E',
          airbnb: '#FF385C',
        },
        violet: {
          accent: '#8B5CF6',
          slack: '#611F69',
        },
      },
      fontFamily: {
        sans: ['Inter', 'Outfit', 'sans-serif'],
        mono: ['JetBrains Mono', 'Fira Code', 'monospace'],
      },
      backdropBlur: {
        xs: '2px',
      },
    },
  },
  plugins: [],
};
