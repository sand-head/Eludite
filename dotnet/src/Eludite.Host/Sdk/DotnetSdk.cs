namespace Eludite.Host.Sdk;

/// <summary>An installed .NET SDK as reported by <c>dotnet --list-sdks</c>.</summary>
/// <param name="Version">The SDK version, e.g. <c>10.0.302</c>.</param>
/// <param name="Path">The directory containing the SDK, e.g. <c>/usr/share/dotnet/sdk</c>.</param>
public sealed record DotnetSdk(string Version, string Path);
