import { expect, test } from "vitest";
import { buildInfo } from "../../../scripts/build-info.mjs";
test("release tag and shared build date", () => {
  expect(
    buildInfo({
      env: {
        GITHUB_REF_TYPE: "tag",
        GITHUB_REF_NAME: "v0.1.0-rc13",
        MOBBOSS_BUILD_DATE: "2026-11-25 04:30 UTC",
      },
      git: () => "02561d7",
    }),
  ).toEqual({
    version: "0.1.0-rc13",
    date: "2026-11-25 04:30 UTC",
    commit: "02561d7",
  });
});
test("no tag or git uses development version and UTC build time", () => {
  expect(
    buildInfo({
      env: { GITHUB_REF_NAME: "main" },
      git: () => {
        throw new Error("no git");
      },
      now: () => new Date("2026-11-25T04:30:59Z"),
    }),
  ).toEqual({
    version: "0.1.0-dev",
    date: "2026-11-25 04:30 UTC",
    commit: "unknown",
  });
});
