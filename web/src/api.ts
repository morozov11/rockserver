/** Shared, credentialed first-party API client. It never persists tokens. */
export type PairingPreview = { request_id: string; device_name: string; platform: string; app_version?: string; verification_phrase: string };
export type ApiError = { code: string; message: string; request_id: string; details: Record<string, unknown> };
export type RegistrationOptions = { challenge_id: string; options: Omit<PublicKeyCredentialCreationOptions, "challenge" | "user"> & { challenge: string; user: Omit<PublicKeyCredentialUserEntity, "id"> & { id: string }; } };
export type AuthenticationOptions = { challenge_id: string; options: Omit<PublicKeyCredentialRequestOptions, "challenge" | "allowCredentials"> & { challenge: string; allowCredentials?: Array<PublicKeyCredentialDescriptor & { id: string }>; } };

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, { ...init, credentials: "same-origin", headers: { "Content-Type": "application/json", ...init.headers } });
  if (!response.ok) throw await response.json() as ApiError;
  return response.status === 204 ? undefined as T : response.json() as Promise<T>;
}

export const api = {
  registrationOptions() { return request<RegistrationOptions>("/v1/auth/passkeys/registration/options", { method: "POST", body: "{}" }); },
  registrationVerify(payload: unknown) { return request<{ user_id: string; csrf_token: string }>("/v1/auth/passkeys/registration/verify", { method: "POST", body: JSON.stringify(payload) }); },
  authenticationOptions() { return request<AuthenticationOptions>("/v1/auth/passkeys/authentication/options", { method: "POST", body: "{}" }); },
  authenticationVerify(payload: unknown) { return request<{ csrf_token: string }>("/v1/auth/passkeys/authentication/verify", { method: "POST", body: JSON.stringify(payload) }); },
  pairing(code: string) { return request<PairingPreview>(`/v1/pairing-requests/lookup?code=${encodeURIComponent(code)}`); },
  approvePairing(requestId: string, approvalSecret: string, verificationPhrase: string, csrfToken: string) {
    return request<void>(`/v1/pairing-requests/${requestId}/approve`, { method: "POST", headers: { "X-CSRF-Token": csrfToken }, body: JSON.stringify({ approval_secret: approvalSecret, verification_phrase: verificationPhrase }) });
  },
};

export const base64url = (bytes: ArrayBuffer | Uint8Array) => {
  const data = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let binary = ""; data.forEach(byte => { binary += String.fromCharCode(byte); });
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};
const decode = (value: string) => { const padded = value.replace(/-/g, "+").replace(/_/g, "/") + "==="; const binary = atob(padded.slice(0, padded.length - (padded.length % 4))); return Uint8Array.from(binary, char => char.charCodeAt(0)); };

/** Converts server JSON options into the browser's WebAuthn request shape. */
export const browserRegistrationOptions = (value: RegistrationOptions): PublicKeyCredentialCreationOptions => ({ ...value.options, challenge: decode(value.options.challenge).buffer, user: { ...value.options.user, id: decode(value.options.user.id).buffer } });
/** Converts a browser registration credential into the API's base64url wire shape. */
export const serializeRegistration = (credential: PublicKeyCredential) => { const response = credential.response as AuthenticatorAttestationResponse; return { id: base64url(credential.rawId), attestationObject: base64url(response.attestationObject), clientDataJSON: base64url(response.clientDataJSON), transports: response.getTransports?.() ?? [] }; };
/** Converts server JSON options into the browser's WebAuthn assertion shape. */
export const browserAuthenticationOptions = (value: AuthenticationOptions): PublicKeyCredentialRequestOptions => ({ ...value.options, challenge: decode(value.options.challenge).buffer, allowCredentials: value.options.allowCredentials?.map(item => ({ ...item, id: decode(item.id).buffer })) });
/** Converts a browser assertion into the API's base64url wire shape. */
export const serializeAuthentication = (credential: PublicKeyCredential) => { const response = credential.response as AuthenticatorAssertionResponse; return { id: base64url(credential.rawId), authenticatorData: base64url(response.authenticatorData), signature: base64url(response.signature), clientDataJSON: base64url(response.clientDataJSON), userHandle: response.userHandle ? base64url(response.userHandle) : null }; };
