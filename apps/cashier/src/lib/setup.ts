import { useQuery } from "@tanstack/react-query";
import { useApiClient } from "@lofi-pos/pos-ui";
import { SetupState } from "@lofi-pos/shared";

/**
 * Polls /admin/setup-state. Until the cashier knows whether the venue
 * has been configured (Owner exists + venue_name set) it can't decide
 * between the first-run screen and the normal PIN-login flow.
 *
 * Refetches on window focus so completing the wizard in another tab
 * (or on a paired phone) re-detects without the operator hunting for
 * a refresh button.
 */
export function useSetupState() {
  const client = useApiClient();
  return useQuery({
    queryKey: ["setup-state"],
    queryFn: () => client.get("/admin/setup-state", SetupState),
    refetchOnWindowFocus: true,
  });
}
