using Microsoft.AspNetCore.Mvc.RazorPages;

namespace BlazorApp.Pages;

public class PrivacyModel : PageModel
{
    public DateTime Updated { get; private set; } = new(2026, 10, 12);

    public void OnGet()
    {
    }
}
