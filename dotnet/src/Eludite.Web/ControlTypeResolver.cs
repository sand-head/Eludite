namespace Niello.Web;

/// <summary>
/// Maps a server control tag to the CLR type the ASP.NET page parser would use for its designer field.
/// Lookup order follows ASP.NET: page <c>Register</c> directives, then web.config <c>&lt;pages&gt;&lt;controls&gt;</c>,
/// then the framework defaults (<see cref="ControlRegistration.FrameworkDefaults"/>). HTML server controls
/// (no prefix) map to <c>System.Web.UI.HtmlControls</c>.
/// </summary>
public sealed class ControlTypeResolver
{
    private readonly IControlTypeCatalog? _catalog;
    private readonly IReadOnlyList<ControlRegistration> _registrations;
    private readonly Func<string, string?>? _userControlType;

    /// <param name="catalog">Types of the project's resolved references; null to trust registrations blindly.</param>
    /// <param name="registrations">Registrations in priority order (page first, then web.config). Framework defaults are appended.</param>
    /// <param name="userControlType">Maps a user-control <c>Src</c> (e.g. <c>~/Controls/Pager.ascx</c>) to its <c>Inherits</c> type.</param>
    public ControlTypeResolver(IControlTypeCatalog? catalog, IEnumerable<ControlRegistration> registrations, Func<string, string?>? userControlType = null)
    {
        ArgumentNullException.ThrowIfNull(registrations);
        _catalog = catalog;
        _registrations = [.. registrations, .. ControlRegistration.FrameworkDefaults];
        _userControlType = userControlType;
    }

    /// <summary>Returns the full type name for <paramref name="control"/>, or null when it cannot be resolved.</summary>
    public string? Resolve(DesignerControl control)
    {
        ArgumentNullException.ThrowIfNull(control);

        if (control.TagPrefix is null)
        {
            return HtmlControlType(control.TagName, control.Attributes.GetValueOrDefault("type"));
        }

        string? unverified = null;
        foreach (var reg in _registrations)
        {
            if (!reg.TagPrefix.Equals(control.TagPrefix, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            if (reg.IsUserControl)
            {
                if (reg.TagName!.Equals(control.TagName, StringComparison.OrdinalIgnoreCase)
                    && _userControlType?.Invoke(reg.Src!) is { } ucType)
                {
                    return ucType;
                }

                continue;
            }

            if (reg.Namespace is null)
            {
                continue;
            }

            var candidate = reg.Namespace + "." + control.TagName;
            if (_catalog is null)
            {
                return candidate;
            }

            if (_catalog.Find(candidate) is { } found)
            {
                return found;
            }

            // The registration names an assembly the catalog never saw (typically a project reference that has not
            // been built): trust it, unless a verified match turns up later.
            if (unverified is null && reg.Assembly is { Length: > 0 } assembly && !_catalog.ContainsAssembly(assembly.Split(',')[0].Trim()))
            {
                unverified = candidate;
            }
        }

        return unverified;
    }

    private static string HtmlControlType(string tagName, string? inputType) => tagName.ToLowerInvariant() switch
    {
        "form" => "System.Web.UI.HtmlControls.HtmlForm",
        "head" => "System.Web.UI.HtmlControls.HtmlHead",
        "title" => "System.Web.UI.HtmlControls.HtmlTitle",
        "meta" => "System.Web.UI.HtmlControls.HtmlMeta",
        "link" => "System.Web.UI.HtmlControls.HtmlLink",
        "a" => "System.Web.UI.HtmlControls.HtmlAnchor",
        "img" => "System.Web.UI.HtmlControls.HtmlImage",
        "button" => "System.Web.UI.HtmlControls.HtmlButton",
        "select" => "System.Web.UI.HtmlControls.HtmlSelect",
        "textarea" => "System.Web.UI.HtmlControls.HtmlTextArea",
        "table" => "System.Web.UI.HtmlControls.HtmlTable",
        "tr" => "System.Web.UI.HtmlControls.HtmlTableRow",
        "td" or "th" => "System.Web.UI.HtmlControls.HtmlTableCell",
        "iframe" => "System.Web.UI.HtmlControls.HtmlIframe",
        "input" => (inputType ?? "text").ToLowerInvariant() switch
        {
            "checkbox" => "System.Web.UI.HtmlControls.HtmlInputCheckBox",
            "radio" => "System.Web.UI.HtmlControls.HtmlInputRadioButton",
            "hidden" => "System.Web.UI.HtmlControls.HtmlInputHidden",
            "file" => "System.Web.UI.HtmlControls.HtmlInputFile",
            "image" => "System.Web.UI.HtmlControls.HtmlInputImage",
            "submit" => "System.Web.UI.HtmlControls.HtmlInputSubmit",
            "reset" => "System.Web.UI.HtmlControls.HtmlInputReset",
            "button" => "System.Web.UI.HtmlControls.HtmlInputButton",
            "password" => "System.Web.UI.HtmlControls.HtmlInputPassword",
            "text" => "System.Web.UI.HtmlControls.HtmlInputText",
            _ => "System.Web.UI.HtmlControls.HtmlInputGenericControl",
        },
        _ => "System.Web.UI.HtmlControls.HtmlGenericControl",
    };
}
