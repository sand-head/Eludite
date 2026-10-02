using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Protocol;

/// <summary>The <c>initialize</c> answer: each capability is true only because the adapter implements it (dap-mono.md).</summary>
public static class Capabilities
{
    public static JObject Initialize() => new()
    {
        ["supportsConfigurationDoneRequest"] = true,
        ["supportsConditionalBreakpoints"] = true,
        ["supportsHitConditionalBreakpoints"] = true,
        ["supportsFunctionBreakpoints"] = true,
        ["supportsLogPoints"] = true,
        ["supportsEvaluateForHovers"] = true,
        ["supportsSetVariable"] = true,
        ["supportsExceptionInfoRequest"] = true,
        ["supportsExceptionFilterOptions"] = true,
        ["supportsTerminateRequest"] = true,
        ["supportTerminateDebuggee"] = true,
        ["supportsDelayedStackTraceLoading"] = true,
        ["exceptionBreakpointFilters"] = new JArray
        {
            new JObject
            {
                ["filter"] = ExceptionSettings.All,
                ["label"] = "All Exceptions",
                ["description"] = "Break when an exception is thrown, handled or not (first chance).",
                ["default"] = false,
                ["supportsCondition"] = true,
                ["conditionDescription"] = "Exception type names, comma-separated (default: System.Exception and every subclass)",
            },
            new JObject
            {
                ["filter"] = ExceptionSettings.UserUnhandled,
                ["label"] = "User-Unhandled Exceptions",
                ["description"] = "Break when user code does not handle an exception.",
                ["default"] = true,
                ["supportsCondition"] = true,
                ["conditionDescription"] = "Exception type names, comma-separated (default: every type)",
            },
        },
    };
}
