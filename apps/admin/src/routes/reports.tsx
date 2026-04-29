/// Reports were moved off-box to the bouncer service. The cashier no longer
/// stores `daily_report` rows, so this route just shows an informational
/// empty state.
export function ReportsRoute() {
  return (
    <div>
      <h1 className="mb-4 text-2xl font-semibold">Daily reports</h1>
      <div className="rounded-lg border bg-white p-6 text-sm text-gray-600 shadow-sm">
        Reports are stored by the bouncer service. Use the bouncer&apos;s own
        reporting UI or API for retrieval.
      </div>
    </div>
  );
}
