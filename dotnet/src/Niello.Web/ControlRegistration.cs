namespace Niello.Web;

/// <summary>
/// A tag-prefix registration, from a <c>&lt;%@ Register %&gt;</c> directive or a web.config
/// <c>&lt;pages&gt;&lt;controls&gt;&lt;add&gt;</c> element. Either a namespace registration
/// (<see cref="Namespace"/>, optional <see cref="Assembly"/>) or a user-control registration
/// (<see cref="TagName"/> plus <see cref="Src"/>).
/// </summary>
public sealed record ControlRegistration(string TagPrefix, string? Namespace, string? Assembly, string? TagName, string? Src)
{
    /// <summary>True for <c>TagName</c> + <c>Src</c> (an <c>.ascx</c> user control).</summary>
    public bool IsUserControl => TagName is not null && Src is not null;

    /// <summary>
    /// The registrations the ASP.NET 4.x root web.config makes for every application
    /// (<c>%WINDIR%\Microsoft.NET\Framework\v4.0.30319\Config\web.config</c>, <c>system.web/pages/controls</c>).
    /// </summary>
    public static IReadOnlyList<ControlRegistration> FrameworkDefaults { get; } =
    [
        new("asp", "System.Web.UI.WebControls", "System.Web", null, null),
        new("asp", "System.Web.UI.WebControls.WebParts", "System.Web", null, null),
        new("asp", "System.Web.UI.WebControls.Expressions", "System.Web.Extensions", null, null),
        new("asp", "System.Web.UI", "System.Web.Extensions", null, null),
        new("asp", "System.Web.UI.WebControls", "System.Web.Extensions", null, null),
        new("asp", "System.Web.DynamicData", "System.Web.DynamicData", null, null),
        new("asp", "System.Web.UI.WebControls", "System.Web.Entity", null, null),
    ];
}
