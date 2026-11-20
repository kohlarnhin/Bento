import animate from 'tailwindcss-animate';

/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        border: 'hsl(var(--border))',
        input: 'hsl(var(--input))',
        ring: 'hsl(var(--ring))',
        background: 'hsl(var(--background))',
        foreground: 'hsl(var(--foreground))',
        primary: { DEFAULT: 'hsl(var(--primary))', foreground: 'hsl(var(--primary-foreground))' },
        destructive: { DEFAULT: 'hsl(var(--destructive))', foreground: 'hsl(var(--destructive-foreground))' },
        muted: { foreground: 'hsl(var(--muted-foreground))' },
        popover: { DEFAULT: 'hsl(var(--popover))', foreground: 'hsl(var(--popover-foreground))' },
        card: { DEFAULT: 'var(--card)', foreground: 'hsl(var(--card-foreground))', border: 'var(--card-border)' },
        success: { foreground: 'hsl(var(--success-foreground))' },
        attention: 'hsl(var(--attention))',
        warning: { foreground: 'hsl(var(--warning-foreground))' },
        terminal: { DEFAULT: 'hsl(var(--terminal-background))', foreground: 'hsl(var(--terminal-foreground))', muted: 'hsl(var(--terminal-muted))', border: 'hsl(var(--terminal-border))' },
      },
      borderWidth: { DEFAULT: '0.5px' },
      borderRadius: {
        control: 'var(--radius-control)',
        lg: 'var(--radius)',
        md: 'calc(var(--radius) - 2px)',
        sm: 'calc(var(--radius) - 4px)',
      },
      fontFamily: {
        sans: 'var(--font-ui)',
        mono: 'var(--font-mono)',
      },
      fontSize: {
        sm: ['var(--text-body)', '1.5'],
        title: ['var(--text-title)', '1.25'],
        meta: ['var(--text-meta)', '1.35'],
        label: ['var(--text-label)', '1.35'],
        keycap: ['var(--text-keycap)', '1.5'],
      },
      fontWeight: {
        heading: 'var(--weight-heading)',
      },
      letterSpacing: {
        title: 'var(--tracking-title)',
      }
    },
  },
  plugins: [animate],
}
