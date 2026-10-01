namespace Eludite.TestBridge;

/// <summary>How the Test Explorer talks to a test project (PLAN.md 4.6).</summary>
public enum TestRunnerProtocol
{
    /// <summary>Microsoft.Testing.Platform server mode (JSON-RPC). Preferred.</summary>
    MicrosoftTestingPlatform,

    /// <summary>The VSTest translation-layer protocol, for projects that have not migrated.</summary>
    VsTest,
}
