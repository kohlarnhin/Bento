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
        secondary: { DEFAULT: 'hsl(var(--secondary))', foreground: 'hsl(var(--secondary-foreground))' },
        destructive: { DEFAULT: 'hsl(var(--destructive))', foreground: 'hsl(var(--destructive-foreground))' },
        muted: { DEFAULT: 'hsl(var(--muted))', foreground: 'hsl(var(--muted-foreground))' },
        accent: { DEFAULT: 'hsl(var(--accent))', foreground: 'hsl(var(--accent-foreground))' },
        popover: { DEFAULT: 'hsl(var(--popover))', foreground: 'hsl(var(--popover-foreground))' },
        card: { DEFAULT: 'var(--card)', foreground: 'hsl(var(--card-foreground))', border: 'var(--card-border)' },
        sidebar: {
          DEFAULT: 'var(--sidebar-background)',
          foreground: 'hsl(var(--sidebar-foreground))',
          primary: 'hsl(var(--sidebar-primary))',
          'primary-foreground': 'hsl(var(--sidebar-primary-foreground))',
          accent: 'hsl(var(--sidebar-accent))',
          'accent-foreground': 'hsl(var(--sidebar-accent-foreground))',
          border: 'hsl(var(--sidebar-border))',
          ring: 'hsl(var(--sidebar-ring))',
        },
        success: { DEFAULT: 'hsl(var(--success))', foreground: 'hsl(var(--success-foreground))', muted: 'hsl(var(--success-muted))' },
        attention: 'hsl(var(--attention))',
        warning: { DEFAULT: 'hsl(var(--warning))', foreground: 'hsl(var(--warning-foreground))', muted: 'hsl(var(--warning-muted))' },
        terminal: { DEFAULT: 'hsl(var(--terminal-background))', foreground: 'hsl(var(--terminal-foreground))', muted: 'hsl(var(--terminal-muted))', border: 'hsl(var(--terminal-border))' },
        mac: {
          bg: '#1e1e24',
          card: '#282933',
          cardHover: '#30313d',
          border: '#3c3d4a',
          accent: '#3b82f6',
        }
      },
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
        brand: ['var(--text-brand)', '1.25'],
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
