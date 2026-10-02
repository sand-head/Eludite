namespace Eludite.Host.Lsp;

/// <summary>Starts an upstream LSP server and hands back its duplex streams.</summary>
public interface ILanguageServerLauncher
{
    Task<LanguageServerConnection> LaunchAsync(CancellationToken cancellationToken);
}

/// <summary>
/// Streams to an upstream LSP server. <see cref="ToServer"/> is written by the host,
/// <see cref="FromServer"/> is read by the host. Disposing <see cref="Lifetime"/> stops the server.
/// </summary>
public sealed record LanguageServerConnection(Stream ToServer, Stream FromServer, IAsyncDisposable Lifetime);
