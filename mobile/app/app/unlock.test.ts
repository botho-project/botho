/**
 * Tests for `classifyUnlockError`, the pure error-classification helper used by
 * the unlock screen's biometric-auth catch block (issue #801).
 *
 * PR #800 (issue #791) introduced a typed `SecureStoreUnavailableError` whose
 * default message tells the user to enable a screen lock. This helper ensures
 * that actionable message reaches the `Alert.alert` shown on the unlock screen,
 * while all other errors keep the existing generic retry message.
 *
 * The helper is a side-effect-free function, so it is unit-tested here in plain
 * jest without rendering the React Native component tree.
 */

import { classifyUnlockError } from "./unlock";
import { SecureStoreUnavailableError } from "../src/native/keychain";

describe("classifyUnlockError", () => {
  it("surfaces the actionable message for SecureStoreUnavailableError", () => {
    const err = new SecureStoreUnavailableError();
    const { title, message } = classifyUnlockError(err);

    expect(title).toBe("Error");
    // Assert against the class's own default message (not a copied string) so
    // this test never drifts from keychain.ts.
    expect(message).toBe(new SecureStoreUnavailableError().message);
  });

  it("preserves a custom SecureStoreUnavailableError message", () => {
    const err = new SecureStoreUnavailableError("custom guidance");
    const { message } = classifyUnlockError(err);

    expect(message).toBe("custom guidance");
  });

  it("returns the generic message for a plain Error", () => {
    const { title, message } = classifyUnlockError(new Error("boom"));

    expect(title).toBe("Error");
    expect(message).toBe("Authentication failed. Please try again.");
  });

  it("returns the generic message for a non-Error thrown value", () => {
    const { title, message } = classifyUnlockError("not an error object");

    expect(title).toBe("Error");
    expect(message).toBe("Authentication failed. Please try again.");
  });
});
