import { useQueryClient } from "@tanstack/react-query";
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";
import { Button } from "@lofi-pos/ui/components/button";
import { LinkQR } from "@lofi-pos/ui/components/link-qr";
import { useSetupState } from "../lib/setup";

/**
 * Shown when /admin/setup-state reports needs_setup=true. Replaces the
 * normal auth/lock flow until the Owner finishes the admin wizard. There
 * is no PIN input, idle lock, or navigation here on purpose — the cashier
 * has nothing to do until setup completes.
 */
export function SetupRequiredRoute() {
  const qc = useQueryClient();
  const { data: state } = useSetupState();
  // Use the LAN URL the server reports so a phone on the same Wi-Fi can
  // open the wizard by scanning the QR. Fall back to the page origin if
  // setup-state hasn't loaded yet (shouldn't happen — App.tsx gates on it).
  const lanBase = state?.lan_url ?? window.location.origin;
  // Admin SPA uses BrowserRouter (basename /ui/admin) — path style, not hash.
  const adminSetupUrl = `${lanBase}/ui/admin/setup`;

  const openInBrowser = async () => {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(adminSetupUrl);
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-gray-50 p-6">
      <div className="max-w-lg w-full text-center space-y-6 p-8 bg-white rounded shadow">
        <div className="space-y-2">
          <h1 className="text-2xl font-semibold">
            <Trans>First-time setup required</Trans>
          </h1>
          <p className="text-gray-600">
            <Trans>
              Welcome to LoFi POS. Before staff can sign in, the venue owner
              needs to complete a short setup: venue name, currency, and the
              Owner PIN.
            </Trans>
          </p>
        </div>
        <LinkQR
          url={adminSetupUrl}
          label={t`Scan to open setup on your phone or laptop`}
        />
        <div className="grid grid-cols-2 gap-3">
          <Button onClick={openInBrowser}>
            <Trans>Open in browser</Trans>
          </Button>
          <Button
            variant="outline"
            onClick={() =>
              qc.invalidateQueries({ queryKey: ["setup-state"] })
            }
          >
            <Trans>I&apos;ve finished setup</Trans>
          </Button>
        </div>
      </div>
    </div>
  );
}
