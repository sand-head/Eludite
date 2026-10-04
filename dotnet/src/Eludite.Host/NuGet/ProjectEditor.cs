using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Microsoft.Build.Construction;
using Microsoft.Build.Evaluation;

namespace Eludite.Host.NuGet;

/// <summary>One edit of a project's packages: set (install, update, consolidate) or remove (uninstall).</summary>
public sealed record PackageEdit(string Id, string? Version, bool Remove);

/// <summary>What an edit of one project wrote.</summary>
public sealed record ProjectEditResult(IReadOnlyList<NuGetEditedFile> Edited, bool CentralPackageManagement);

/// <summary>
/// Edits <c>PackageReference</c> items, and <c>PackageVersion</c> items of <c>Directory.Packages.props</c> under Central
/// Package Management, with MSBuild's construction model (<see cref="ProjectRootElement"/> opened with
/// <c>preserveFormatting</c>): only the item that changes is touched, so comments, indentation and element order stay.
/// The SDK's MSBuild is loaded by Microsoft.Build.Locator; no Microsoft.Build type is touched before it registers.
/// </summary>
public static class ProjectEditor
{
    /// <summary>Applies <paramref name="edits"/> to the project at <paramref name="projectPath"/> and saves what changed.</summary>
    /// <exception cref="InvalidOperationException">No .NET SDK is registered, or the project cannot be read.</exception>
    public static ProjectEditResult Apply(string projectPath, IReadOnlyList<PackageEdit> edits)
    {
        ArgumentNullException.ThrowIfNull(edits);
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            throw new InvalidOperationException("no .NET SDK found by Microsoft.Build.Locator; project files cannot be edited");
        }

        // One edit at a time: MSBuild's in-process state is shared with the tree's evaluation.
        lock (Projects.MsBuildProjectTreeEvaluator.EvaluationLock)
        {
            return ApplyGuarded(Path.GetFullPath(projectPath), edits);
        }
    }

    /// <summary>MSBuild's project errors as <see cref="InvalidOperationException"/>, so callers never name an MSBuild type.</summary>
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static ProjectEditResult ApplyGuarded(string projectPath, IReadOnlyList<PackageEdit> edits)
    {
        try
        {
            return ApplyCore(projectPath, edits);
        }
        catch (Microsoft.Build.Exceptions.InvalidProjectFileException ex)
        {
            throw new InvalidOperationException(ex.Message, ex);
        }
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static ProjectEditResult ApplyCore(string projectPath, IReadOnlyList<PackageEdit> edits)
    {
        using var collection = new ProjectCollection();
        var root = ProjectRootElement.Open(projectPath, collection, preserveFormatting: true);
        var (cpm, propsPath) = CentralManagement(root, collection);
        var props = cpm && propsPath is not null && File.Exists(propsPath)
            ? ProjectRootElement.Open(propsPath, collection, preserveFormatting: true)
            : null;
        var projectChanges = new List<string>();
        var propsChanges = new List<string>();
        foreach (var edit in edits)
        {
            var item = Find(root, "PackageReference", edit.Id);
            if (edit.Remove)
            {
                if (item is not null)
                {
                    RemoveItem(item);
                    projectChanges.Add($"PackageReference {edit.Id} removed");
                }

                continue;
            }

            var version = edit.Version ?? throw new InvalidOperationException($"no version for {edit.Id}");
            if (props is not null)
            {
                // Central Package Management: a version-less PackageReference here, the version in the props file (or
                // the project's VersionOverride when it has one).
                if (item is null)
                {
                    AddItem(root, "PackageReference", edit.Id, null);
                    projectChanges.Add($"PackageReference {edit.Id} added");
                }
                else if (FindMetadata(item, "VersionOverride") is { } overridden)
                {
                    if (overridden.Value != version)
                    {
                        overridden.Value = version;
                        projectChanges.Add($"PackageReference {edit.Id} VersionOverride {version}");
                    }

                    continue;
                }

                var central = Find(props, "PackageVersion", edit.Id);
                if (central is null)
                {
                    AddItem(props, "PackageVersion", edit.Id, version);
                    propsChanges.Add($"PackageVersion {edit.Id} {version} added");
                }
                else if (SetVersion(central, version))
                {
                    propsChanges.Add($"PackageVersion {edit.Id} {version}");
                }

                continue;
            }

            if (item is null)
            {
                AddItem(root, "PackageReference", edit.Id, version);
                projectChanges.Add($"PackageReference {edit.Id} {version} added");
            }
            else if (SetVersion(item, version))
            {
                projectChanges.Add($"PackageReference {edit.Id} {version}");
            }
        }

        var edited = new List<NuGetEditedFile>();
        if (projectChanges.Count > 0)
        {
            root.Save();
            edited.Add(new NuGetEditedFile(projectPath, "project", projectChanges));
        }

        if (props is not null && propsChanges.Count > 0)
        {
            props.Save();
            edited.Add(new NuGetEditedFile(props.FullPath, "centralPackageVersions", propsChanges));
        }

        return new ProjectEditResult(edited, cpm);
    }

    /// <summary>Whether the project manages versions centrally, and its Directory.Packages.props.</summary>
    private static (bool Cpm, string? PropsPath) CentralManagement(ProjectRootElement root, ProjectCollection collection)
    {
        try
        {
            const ProjectLoadSettings settings = ProjectLoadSettings.IgnoreMissingImports | ProjectLoadSettings.IgnoreEmptyImports | ProjectLoadSettings.IgnoreInvalidImports;
            var project = new Project(root, globalProperties: null, toolsVersion: null, collection, settings);
            var cpm = string.Equals(project.GetPropertyValue("ManagePackageVersionsCentrally"), "true", StringComparison.OrdinalIgnoreCase);
            var props = project.GetPropertyValue("DirectoryPackagesPropsPath");
            collection.UnloadProject(project);
            return (cpm, cpm ? (props.Length > 0 ? props : InstalledReader.FindPropsFile(root.FullPath)) : null);
        }
        catch (Microsoft.Build.Exceptions.InvalidProjectFileException)
        {
            var props = InstalledReader.FindPropsFile(root.FullPath);
            var cpm = InstalledReader.ManagesCentrally(System.Xml.Linq.XDocument.Load(root.FullPath), props);
            return (cpm, cpm ? props : null);
        }
    }

    private static ProjectItemElement? Find(ProjectRootElement root, string itemType, string id) =>
        root.Items.FirstOrDefault(i => i.ItemType == itemType && string.Equals(i.Include, id, StringComparison.OrdinalIgnoreCase));

    private static ProjectMetadataElement? FindMetadata(ProjectItemElement item, string name) =>
        item.Metadata.FirstOrDefault(m => string.Equals(m.Name, name, StringComparison.OrdinalIgnoreCase));

    /// <summary>Sets the item's Version (attribute or element, as written); true when it changed.</summary>
    private static bool SetVersion(ProjectItemElement item, string version)
    {
        if (FindMetadata(item, "Version") is { } existing)
        {
            if (existing.Value == version)
            {
                return false;
            }

            existing.Value = version;
            return true;
        }

        item.AddMetadata("Version", version, expressAsAttribute: true);
        return true;
    }

    /// <summary>Adds an item to the first unconditioned ItemGroup holding items of its type, else to a new ItemGroup.</summary>
    private static void AddItem(ProjectRootElement root, string itemType, string id, string? version)
    {
        var group = root.ItemGroups.FirstOrDefault(g => g.Condition.Length == 0 && g.Items.Any(i => i.ItemType == itemType))
            ?? root.AddItemGroup();
        var item = group.AddItem(itemType, id);
        if (version is not null)
        {
            item.AddMetadata("Version", version, expressAsAttribute: true);
        }
    }

    private static void RemoveItem(ProjectItemElement item)
    {
        var group = item.Parent;
        group.RemoveChild(item);
        if (group is ProjectItemGroupElement { Count: 0 } empty)
        {
            empty.Parent.RemoveChild(empty);
        }
    }
}
