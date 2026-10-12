using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Microsoft.Build.Construction;
using Microsoft.Build.Evaluation;

namespace Eludite.Host.Projects;

/// <summary>An item to add to a project file: <see cref="Include"/> relative to the project with backslashes; <see cref="Link"/> as an attribute.</summary>
public sealed record ProjectItemToAdd(string ItemType, string Include)
{
    public string? Link { get; init; }
}

/// <summary>
/// Adds items to a project file on MSBuild's construction model with formatting preserved (brief 0072, Add New Item
/// and Add Existing Item; the same parse, save and <see cref="TextFileFormat"/> layout as brief 0049's
/// <see cref="ProjectFileEditor"/>): each item goes into the first unconditioned <c>ItemGroup</c> holding items of its
/// type, else a new one; an item whose <c>Include</c> is already listed with that type is not added again.
/// </summary>
public static class ProjectItemWriter
{
    /// <summary>Adds <paramref name="items"/>; true when the file changed. -32602 when no MSBuild is found.</summary>
    public static bool Add(string projectPath, IReadOnlyList<ProjectItemToAdd> items, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        ArgumentNullException.ThrowIfNull(items);
        if (items.Count == 0)
        {
            return false;
        }

        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            throw HostErrors.BadParams("no .NET SDK found by Microsoft.Build.Locator: project items need MSBuild");
        }

        return AddCore(Path.GetFullPath(projectPath), items, cancellationToken);
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static bool AddCore(string projectPath, IReadOnlyList<ProjectItemToAdd> items, CancellationToken cancellationToken)
    {
        var bytes = File.ReadAllBytes(projectPath);
        var format = TextFileFormat.Detect(bytes, out var text);
        using var collection = new ProjectCollection(null, null, ToolsetDefinitionLocations.Default);
        try
        {
            var root = ProjectFileEditor.Parse(text, collection, preserveFormatting: true);
            root.FullPath = projectPath;
            var changed = false;
            foreach (var item in items)
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (root.Items.Any(i => string.Equals(i.ItemType, item.ItemType, StringComparison.OrdinalIgnoreCase)
                    && string.Equals(i.Include.Replace('/', '\\').Trim(), item.Include, StringComparison.OrdinalIgnoreCase)))
                {
                    continue;
                }

                var group = root.ItemGroups.FirstOrDefault(g => g.Condition.Length == 0 && g.Items.Any(i => string.Equals(i.ItemType, item.ItemType, StringComparison.OrdinalIgnoreCase)))
                    ?? root.AddItemGroup();
                var element = group.AddItem(item.ItemType, item.Include);
                if (item.Link is { } link)
                {
                    element.AddMetadata("Link", link, expressAsAttribute: true);
                }

                changed = true;
            }

            if (!changed)
            {
                return false;
            }

            var written = format.Encode(ProjectFileEditor.Serialize(root));
            if (written.AsSpan().SequenceEqual(bytes))
            {
                return false;
            }

            cancellationToken.ThrowIfCancellationRequested();
            TextFileFormat.WriteAtomically(projectPath, written);
            return true;
        }
        finally
        {
            collection.UnloadAllProjects();
        }
    }
}
