using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;

namespace Eludite.Web;

/// <summary>Answers "does this type exist" for control type resolution.</summary>
public interface IControlTypeCatalog
{
    /// <summary>Returns the correctly cased full name of a public type, matched case-insensitively, or null.</summary>
    string? Find(string fullName);

    /// <summary>
    /// True when the catalog indexed an assembly with this simple name. A registration for an assembly the catalog
    /// has not seen (a project reference that is not built yet) cannot be verified, only trusted.
    /// </summary>
    bool ContainsAssembly(string assemblyName) => true;
}

/// <summary>
/// The public top-level type names of a set of assemblies, read with System.Reflection.Metadata
/// (no assembly loading, works on reference assemblies).
/// </summary>
public sealed class MetadataTypeCatalog : IControlTypeCatalog
{
    private readonly Dictionary<string, string> _types = new(StringComparer.OrdinalIgnoreCase);
    private readonly HashSet<string> _assemblies = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>Indexes <paramref name="assemblyPaths"/>; unreadable files are skipped and listed in <see cref="Skipped"/>.</summary>
    public MetadataTypeCatalog(IEnumerable<string> assemblyPaths)
    {
        ArgumentNullException.ThrowIfNull(assemblyPaths);
        var skipped = new List<string>();
        foreach (var path in assemblyPaths)
        {
            try
            {
                using var stream = File.OpenRead(path);
                using var pe = new PEReader(stream);
                if (!pe.HasMetadata)
                {
                    skipped.Add(path);
                    continue;
                }

                var md = pe.GetMetadataReader();
                if (md.IsAssembly)
                {
                    _assemblies.Add(md.GetString(md.GetAssemblyDefinition().Name));
                }

                foreach (var handle in md.TypeDefinitions)
                {
                    var type = md.GetTypeDefinition(handle);
                    if ((type.Attributes & TypeAttributes.VisibilityMask) != TypeAttributes.Public)
                    {
                        continue;
                    }

                    var ns = md.GetString(type.Namespace);
                    var name = md.GetString(type.Name);
                    var full = ns.Length == 0 ? name : ns + "." + name;
                    _types.TryAdd(full, full);
                }
            }
            catch (Exception ex) when (ex is IOException or BadImageFormatException or UnauthorizedAccessException)
            {
                skipped.Add(path);
            }
        }

        Skipped = skipped;
    }

    /// <summary>Assemblies that could not be read.</summary>
    public IReadOnlyList<string> Skipped { get; }

    /// <summary>Number of public types indexed.</summary>
    public int Count => _types.Count;

    /// <inheritdoc />
    public string? Find(string fullName) => _types.GetValueOrDefault(fullName);

    /// <inheritdoc />
    public bool ContainsAssembly(string assemblyName) => _assemblies.Contains(assemblyName);
}
