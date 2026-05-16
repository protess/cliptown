/**
 * P4 F1: opencode model spec parsing contract.
 *
 * `OPENCODE_MODEL` accepts `provider/model` and bare `model` forms.
 * The docs in DEPLOY.md tell operators `OPENCODE_MODEL=ollama/llama3.1`
 * routes to a local ollama instance — this test pins that contract so
 * a refactor of `splitProviderModel` can't silently break the local-LLM
 * happy path.
 */
import { describe, it, expect } from "vitest";
import { splitProviderModel } from "../src/index.js";

describe("splitProviderModel", () => {
  it("treats a bare model as openai-provider", () => {
    expect(splitProviderModel("gpt-5-mini")).toEqual({
      providerID: "openai",
      modelID: "gpt-5-mini",
    });
  });

  it("splits ollama/<model> on the first slash", () => {
    expect(splitProviderModel("ollama/llama3.1")).toEqual({
      providerID: "ollama",
      modelID: "llama3.1",
    });
  });

  it("preserves slashes after the first separator (e.g. ollama/qwen2.5:7b)", () => {
    expect(splitProviderModel("ollama/qwen2.5:7b")).toEqual({
      providerID: "ollama",
      modelID: "qwen2.5:7b",
    });
  });

  it("handles other providers (anthropic, vllm, lm-studio)", () => {
    expect(splitProviderModel("anthropic/claude-haiku-4-5")).toEqual({
      providerID: "anthropic",
      modelID: "claude-haiku-4-5",
    });
    expect(splitProviderModel("vllm/llama3.3")).toEqual({
      providerID: "vllm",
      modelID: "llama3.3",
    });
    expect(splitProviderModel("lm-studio/qwen2.5")).toEqual({
      providerID: "lm-studio",
      modelID: "qwen2.5",
    });
  });

  it("empty modelID after slash is preserved verbatim (caller validates)", () => {
    // We don't try to be clever — the upstream opencode CLI rejects empty
    // model names with a clearer error than we could produce here.
    expect(splitProviderModel("ollama/")).toEqual({
      providerID: "ollama",
      modelID: "",
    });
  });
});
