/// <reference types="vite/client" />

interface Window {
  AetherVscodexEmbed?: {
    active: boolean;
    stop?: () => void;
  };
  VscodexI18n?: {
    locale: () => string;
    setLocale: (locale: string, options?: { persist?: boolean }) => string;
  };
}
